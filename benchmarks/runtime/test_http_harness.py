"""Deterministic tests of the issue-481 controller and analysis (not legacy v2)."""
import contextlib
import copy
import importlib.util
import io
import itertools
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import controller


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


http = module('http_summary_test', 'summarize-http.py')
paired = module('paired_summary_test', 'summarize-comparisons.py')


def process(ticks=0, writes=0, rss=1024):
    fields = ['0'] * 50
    fields[0] = 'R'
    fields[11], fields[12] = str(ticks), '0'
    # Reaped-child CPU must not enter product self CPU accounting.
    fields[13], fields[14] = '1000000', '2000000'
    return {'stat': '123 (name with ) parentheses) ' + ' '.join(fields),
            'io': f'write_bytes: {writes}\nread_bytes: 0\n',
            'smaps_rollup': f'Rss: {rss} kB\nPss: {rss-128} kB\n'}


def sample():
    wire = [{'body': {'model': 'chat-model'}}]
    return {'measurement_version': 3, 'case': 'cancel', 'history': 0, 'measured': 1,
            'oracle': 'passed', 'wire_requests': wire,
            'live_requests': [{'messages': []}], 'reopened_requests': [{'messages': []}],
            'verified_live_requests': 1, 'verified_reopened_requests': 1,
            'samples': [{'turn': 0, 'measured': True, 'settled_us': 1000,
                         'accepted_us': 10, 'process': process(10, 1024)}],
            'lifecycle': [{'phase': name, 'elapsed_us': i*1000,
                           'process': process(i*10, i*1024)}
                          for i,name in enumerate(controller.RUST_PHASES)]}


class FakeProcess:
    def __init__(self, name, stdout, events, exit_code=0):
        self.name, self.stdout = name, io.StringIO(stdout)
        self.stdin, self.returncode = io.StringIO(), None
        self.pid = 1001 if name == 'product' else 1002
        self.events, self.exit_code = events, exit_code
        self.before_wait = lambda: None

    def poll(self):
        return self.returncode

    def wait(self):
        self.before_wait()
        self.events.append(self.name + '_exit')
        if self.returncode is None:
            self.returncode = self.exit_code
        return self.returncode


class ControllerTests(unittest.TestCase):
    def exercise(self, mutate=lambda value: None, provider_exit=0, startup_failure=False):
        with tempfile.TemporaryDirectory() as directory:
            stem = Path(directory)/'sample'
            value = sample()
            mutate(value)
            events = []
            ready = {'scenario': 'runtime_benchmark_cancel', 'workspace': '/fixture'}
            product = FakeProcess('product', 'RUSTX_BENCH_READY '+json.dumps(ready)+'\n'
                                  +f'test {controller.TEST} ... ok\n', events)
            provider = FakeProcess('provider', json.dumps({'ready': True, 'host': '127.0.0.1', 'port': 1234})+'\n', events, provider_exit)
            def product_exit():
                self.assertIsNone(provider.returncode, 'provider must survive cleanup/product exit')
                events.append('product_cleanup_evidence')
                stem.with_suffix('.json').write_text(json.dumps(value))
            product.before_wait = product_exit
            calls = []
            def popen(argv, **kwargs):
                self.assertTrue(kwargs['start_new_session'])
                calls.append(argv)
                if len(calls) == 1:
                    events.append('product_start')
                    return product
                self.assertEqual(argv[0], 'uv')
                if startup_failure:
                    raise OSError('provider launch refused')
                events.append('provider_start')
                return provider
            def control(base, path):
                self.assertEqual(product.returncode, 0)
                self.assertIsNone(provider.returncode)
                self.assertEqual(path, '/__control/shutdown')
                events.append('provider_shutdown')
                return {'ok': True, 'requestCount': 1, 'requests': sample()['wire_requests']}
            def kill(pid, signal):
                events.append('kill_'+str(pid))
                (product if pid == product.pid else provider).returncode = -9
            clock = itertools.count(0, 1000)
            with patch.object(controller.subprocess, 'Popen', side_effect=popen), \
                 patch.object(controller, 'control', side_effect=control), \
                 patch.object(controller.os, 'killpg', side_effect=kill), \
                 patch.object(controller.time, 'monotonic_ns', side_effect=lambda: next(clock)):
                record = controller.run(['product'], stem, 'cancel', {}, Path('/provider'))
            persisted = json.loads(stem.with_suffix('.process.json').read_text())
            self.assertEqual(record, persisted)
            self.assertTrue(stem.with_suffix('.log').exists())
            self.assertTrue(product.stdout.closed and product.stdin.closed)
            return record, events

    def test_sibling_ownership_cleanup_precedes_provider_shutdown_and_reaping(self):
        record, events = self.exercise()
        self.assertTrue(record['accepted'])
        self.assertEqual(events, ['product_start','provider_start','product_cleanup_evidence',
                                  'product_exit','provider_shutdown','provider_exit'])
        self.assertEqual([record[k] for k in ('product_startup_ns','provider_startup_ns',
                         'through_product_exit_ns','provider_teardown_ns','complete_lifecycle_ns')],
                         [1000,1000,4000,1000,7000])

    def test_rejected_oracle_is_retained_after_ordered_shutdown(self):
        record, events = self.exercise(lambda s: s.update(oracle='failed'))
        self.assertFalse(record['accepted'])
        self.assertIn('AssertionError', record['error'])
        self.assertLess(events.index('product_exit'),events.index('provider_shutdown'))

    def test_wire_mismatch_and_failed_provider_exit_are_rejected(self):
        record, _ = self.exercise(lambda s: s['wire_requests'][0]['body'].update(model='different'))
        self.assertFalse(record['accepted'])
        record, _ = self.exercise(provider_exit=1)
        self.assertFalse(record['accepted'])

    def test_provider_start_failure_kills_only_started_product_and_records_failure(self):
        record, events = self.exercise(startup_failure=True)
        self.assertFalse(record['accepted'])
        self.assertIn('provider launch refused',record['error'])
        self.assertIn('kill_1001',events)
        self.assertNotIn('provider_shutdown',events)
        self.assertNotIn('kill_1002',events)

    def test_missing_duplicate_reordered_and_nonmonotonic_lifecycle_rejected(self):
        mutations = [lambda s: s['lifecycle'].pop(),
                     lambda s: s['lifecycle'].insert(2,copy.deepcopy(s['lifecycle'][1])),
                     lambda s: s['lifecycle'].reverse(),
                     lambda s: s['lifecycle'][3].update(elapsed_us=0)]
        for mutate in mutations:
            with self.subTest(mutate=mutate):
                value=sample();mutate(value)
                with self.assertRaises(AssertionError):controller.validate_lifecycle(value)

    def test_pi_lifecycle_is_complete_without_rust_only_close_phases(self):
        value=sample();del value['measurement_version']
        value['lifecycle']=[p for p in value['lifecycle'] if p['phase'] in controller.PI_PHASES]
        self.assertEqual(list(controller.validate_lifecycle(value)),controller.PI_PHASES)


class SummaryTests(unittest.TestCase):
    def setUp(self):
        self.directory=tempfile.TemporaryDirectory();self.addCleanup(self.directory.cleanup)
        self.root=Path(self.directory.name)
        self.clock=patch.object(http.os,'sysconf',return_value=100);self.clock.start();self.addCleanup(self.clock.stop)

    def write(self, name, value, accepted=True):
        path=self.root/(name+'.json');path.write_text(json.dumps(value))
        path.with_name(path.stem+'.process.json').write_text(json.dumps({'accepted':accepted,'case':value.get('case')}))
        return path

    def test_self_cpu_seed_exclusion_writes_and_sampled_memory_endpoints(self):
        value=sample();value.update(history=2,measured=2)
        value['samples']=[{'turn':i,'measured':i>=2,'settled_us':n,'accepted_us':1,
                          'process':process(t,1024*i,8192 if i==2 else 1024)}
                         for i,(n,t) in enumerate([(900000,10),(800000,30),(10000,50),(30000,70)])]
        result=http.sample(self.write('metrics',value))
        self.assertEqual(result['mean_turn_ms'],20)
        self.assertEqual(result['turn_cpu_ms_per_turn'],200)
        self.assertEqual(result['turn_write_mib_per_turn'],1/1024)
        self.assertEqual(result['lifecycle_cpu_ms'],900)
        self.assertEqual(result['max_sampled_rss_mib'],8)
        self.assertEqual(result['phases'][-1]['through'],'after_cleanup')

    def test_unavailable_counters_are_none_not_zero(self):
        p={'process':{'stat':None,'io':None,'smaps_rollup':None}}
        self.assertEqual(http.counters(p),dict.fromkeys(('cpu_ms','write_bytes','rss_kib','pss_kib','anon_pss_kib')))
        self.assertIsNone(http.delta(p,p,'cpu_ms'))

    def pairs(self, before=(1,100,101), after=(2,99,101)):
        for i,(b,a) in enumerate(zip(before,after)):
            for side,time_ms in (('before',b),('after',a)):
                value=sample();value['samples'][0]['settled_us']=time_ms*1000
                self.write(f'{i}-cancel-{side}',value)

    def test_pairwise_ratio_is_not_ratio_of_unrelated_medians(self):
        self.pairs();result=paired.summarize([self.root])
        values=result['medians']['cancel']
        self.assertEqual(values['before']['mean_turn_ms'],100)
        self.assertEqual(values['after']['mean_turn_ms'],99)
        self.assertEqual(values['after_over_before']['mean_turn_ms'],1)
        self.assertEqual(len(result['samples']['cancel']),3)

    def test_missing_duplicate_empty_and_rejected_pairs_fail_closed(self):
        with self.assertRaises(ValueError):paired.summarize([self.root])
        self.pairs()
        with self.assertRaises(ValueError):paired.summarize([self.root,self.root])
        path=self.root/'0-cancel-after.json';saved=path.read_text();path.unlink()
        with self.assertRaises(ValueError):paired.summarize([self.root])
        path.write_text(saved)
        self.write('0-cancel-after',sample(),accepted=False)
        with self.assertRaises(AssertionError):paired.summarize([self.root])
        self.assertFalse(json.loads(path.with_name(path.stem+'.process.json').read_text())['accepted'])

    def test_workload_case_and_executable_mismatches_rejected(self):
        self.pairs();path=self.root/'0-cancel-after.json';value=json.loads(path.read_text())
        value['history']=1;path.write_text(json.dumps(value))
        with self.assertRaises((ValueError,IndexError)):paired.summarize([self.root])
        value['history']=0;value['case']='tool';path.write_text(json.dumps(value))
        with self.assertRaises(AssertionError):paired.summarize([self.root])
        value['case']='cancel';path.write_text(json.dumps(value))
        other=self.root/'other';other.mkdir()
        for folder,digest in ((self.root,'first'),(other,'second')):
            (folder/'manifest.json').write_text(json.dumps({'executables':{n:{'sha256':digest} for n in ('before','after')}}))
        with self.assertRaises(ValueError):paired.summarize([self.root,other])

    def test_zero_denominator_does_not_fabricate_ratio(self):
        self.pairs((0,),(2,));result=paired.summarize([self.root])
        self.assertNotIn('mean_turn_ms',result['samples']['cancel'][0]['after_over_before'])

    def test_http_cli_retains_failed_sample_in_failed_group(self):
        self.write('0-cancel-rust',sample(),accepted=False)
        output=io.StringIO()
        with patch('sys.argv',['summary',str(self.root)]),contextlib.redirect_stdout(output):http.main()
        result=json.loads(output.getvalue())
        self.assertEqual(len(result['groups']['failed']),1)

    def test_append_endpoints_and_footprints(self):
        phases=['before_initialization','after_seed','after_appends','after_close','after_reopen_verification','after_cleanup']
        value={'history':64,'measured':32,'append_ns':[2000000]*32,
               'lifecycle':[{'phase':name,'elapsed_us':i,'process':process(i*10,i*4096)} for i,name in enumerate(phases)],
               'before_close':[{'path':'/store/a.sqlite-wal','logical':1025,'allocated':4096}],
               'closed_footprint':[{'path':'/store/a.sqlite','logical':8192,'allocated':8192}]}
        for side in ('before','after'):self.write('0-64-'+side,value)
        result=paired.summarize([self.root],append=True)['medians']['64']['before']
        self.assertEqual(result['mean_append_ms'],2)
        self.assertEqual(result['append_cpu_ms_per_append'],100/32)
        self.assertEqual(result['append_write_mib_per_append'],4096/(32*2**20))
        self.assertEqual(result['before_close_wal_logical'],1025)
        self.assertEqual(result['before_close_wal_allocated'],4096)
        self.assertEqual(result['closed_footprint_wal_allocated'],0)

    def test_cold_append_operation_counters_do_not_include_reopens(self):
        self.test_append_endpoints_and_footprints()
        for side in ('before','after'):
            path=self.root/f'0-64-{side}.json';value=json.loads(path.read_text())
            value.update(cold=True,operation_counters=[{'before':process(10,100),'after':process(11,200)}]*32)
            path.write_text(json.dumps(value))
        result=paired.summarize([self.root],append=True)['medians']['64']['before']
        self.assertEqual(result['append_cpu_ms_per_append'],10)
        self.assertEqual(result['append_write_mib_per_append'],100/2**20)
        self.assertEqual(result['append_phase_with_reopens_cpu_ms_per_append'],100/32)
        path=self.root/'0-64-after.json';value=json.loads(path.read_text());value['cold']=False
        path.write_text(json.dumps(value))
        with self.assertRaises(ValueError):paired.summarize([self.root],append=True)
        value['cold']=True;value['operation_counters'].pop();path.write_text(json.dumps(value))
        with self.assertRaises(ValueError):paired.summarize([self.root],append=True)

    def test_anonymous_memory_is_separate_and_unavailable_is_not_zero(self):
        p=process();self.assertIsNone(http.counters({'process':p})['anon_pss_kib'])
        p['smaps_rollup']+='Pss_Anon: 256 kB\n'
        self.assertEqual(http.counters({'process':p})['anon_pss_kib'],256)


if __name__ == '__main__':unittest.main()
