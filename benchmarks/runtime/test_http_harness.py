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


class ObserverModeTests(unittest.TestCase):
    def test_new_modes_require_matching_raw_and_per_turn_identity(self):
        for mode in controller.OBSERVER_MODES[1:]:
            value=sample()
            result={'product_exit':0,'provider_exit':0,'observer_mode':mode,
                    'provider_report':{'ok':True,'requestCount':1,'requests':value['wire_requests']}}
            check=lambda: controller.validate_sample(result,value,'cancel',f'test {controller.TEST} ... ok')
            with self.assertRaises(AssertionError): check()
            value['observer_mode']=mode
            with self.assertRaises(AssertionError): check()
            value['samples'][0]['observer_mode']=mode
            value['measurement_version']=4
            phases=list(controller.RUST_PHASES)
            phases.insert(phases.index('after_turns')+1,'after_late_snapshot')
            value['lifecycle']=[{'phase':name,'elapsed_us':i*1000,'process':process(i*10)} for i,name in enumerate(phases)]
            value['canonical']=[]
            value['late_snapshot']={'messages':[],'checkpoint':{'cursor':1,'read_cut':{'source_cut':{'ledger_through':0}}},'replay':{'kind':'contiguous','through':1}}
            value['observer_validation']={'completed_attempts':1,'text':[],'tool_results':[]}
            check()
            for key in ['messages','checkpoint','replay']:
                saved=value['late_snapshot'].pop(key)
                with self.assertRaises((KeyError,AssertionError)): check()
                value['late_snapshot'][key]=saved
            value['late_snapshot']['messages']=[{'changed':True}]
            with self.assertRaises(AssertionError): check()
            value['late_snapshot']['messages']=[]
            value['late_snapshot']['replay']['through']=2
            with self.assertRaises(AssertionError): check()
            value['late_snapshot']['replay']['through']=1
            if mode=='live-subscriber':
                value['live_requests'][0]['snapshot']={'provisional_message_id':'partial-owner'}
                value['reopened_requests']=copy.deepcopy(value['live_requests'])
                value['observer_validation']['text']=[{'message_id':'partial-owner','block_index':'ContentBlockIndex(0)','text':'partial'}]
                check()
                value['observer_validation']['text'][0]['text']='must not appear'
                with self.assertRaises(AssertionError): check()
                value['observer_validation']['text'][0]['text']='partial'
                value['observer_validation']['text'][0]['message_id']='wrong-owner'
                with self.assertRaises(AssertionError): check()
                value['observer_validation']['text']=[]
            value['observer_mode']='native-workflow'
            with self.assertRaises(AssertionError): check()

    def test_aligned_modes_require_late_snapshot_phase(self):
        value=sample(); value['observer_mode']='no-live-subscriber'
        with self.assertRaises(AssertionError): controller.validate_lifecycle(value)


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

    def test_aligned_startup_and_complete_self_cpu_remain_separate(self):
        value=sample(); value['observer_mode']='no-live-subscriber'
        index=next(i for i,p in enumerate(value['lifecycle']) if p['phase']=='after_turns')+1
        late=copy.deepcopy(value['lifecycle'][index-1]); late['phase']='after_late_snapshot'
        value['lifecycle'].insert(index,late)
        for i,point in enumerate(value['lifecycle']): point['process']=process(100+i*10,1000+i*100)
        result=http.sample(self.write('startup',value))
        self.assertEqual(result['startup_self_cpu_ms'],1000)
        self.assertEqual(result['through_cleanup_self_cpu_ms'],2000)
        self.assertEqual(result['lifecycle_cpu_ms'],1000)
        self.assertEqual(result['through_cleanup_process_write_mib'],2000/2**20)
        self.assertEqual(result['lifecycle_write_mib'],1000/2**20)

    def test_unavailable_counters_are_none_not_zero(self):
        p={'process':{'stat':None,'io':None,'smaps_rollup':None}}
        self.assertEqual(http.counters(p),dict.fromkeys(('cpu_ms','write_bytes','rss_kib','pss_kib','anon_pss_kib')))
        self.assertIsNone(http.delta(p,p,'cpu_ms'))

    def pairs(self, before=(1,100,101), after=(2,99,101)):
        for i,(b,a) in enumerate(zip(before,after)):
            for side,time_ms in (('before',b),('after',a)):
                value=sample();value['samples'][0]['settled_us']=time_ms*1000
                self.write(f'{i}-cancel-{side}',value)

    def set_pair_mode(self, pair, mode):
        for side in ('before', 'after'):
            path=self.root/f'{pair}-cancel-{side}.json'
            value=json.loads(path.read_text()); value['observer_mode']=mode
            value['measurement_version']=4
            for turn in value['samples']: turn['observer_mode']=mode
            index=next(i for i,p in enumerate(value['lifecycle']) if p['phase']=='after_turns')+1
            late=copy.deepcopy(value['lifecycle'][index-1]); late['phase']='after_late_snapshot'
            value['lifecycle'].insert(index,late)
            path.write_text(json.dumps(value))
            path.with_name(path.stem+'.process.json').write_text(json.dumps({'accepted':True,'observer_mode':mode}))

    def test_observer_modes_cannot_mix_within_or_between_pairs(self):
        self.pairs(); self.set_pair_mode(0,'live-subscriber')
        with self.assertRaisesRegex(ValueError,'mixed observer modes'):
            paired.summarize([self.root])
        for i in (1,2): self.set_pair_mode(i,'live-subscriber')
        self.assertEqual(paired.summarize([self.root])['observer_mode'],'live-subscriber')
        path=self.root/'0-cancel-after.process.json'
        path.write_text(json.dumps({'accepted':True,'observer_mode':'no-live-subscriber'}))
        with self.assertRaisesRegex(AssertionError,'observer mode'):
            paired.summarize([self.root])

    def test_manifest_observer_mode_must_match_raw_samples(self):
        self.pairs()
        for i in range(3): self.set_pair_mode(i,'no-live-subscriber')
        (self.root/'manifest.json').write_text(json.dumps({'observer_mode':'live-subscriber'}))
        with self.assertRaisesRegex(ValueError,'manifest observer mode'):
            paired.summarize([self.root])

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

    def test_append_modes_cannot_mix_across_pairs_or_directories(self):
        self.test_append_endpoints_and_footprints()
        self.assertEqual(paired.summarize([self.root],append=True)['append_mode'],'warm')
        for side in ('before','after'):
            value=json.loads((self.root/f'0-64-{side}.json').read_text())
            value.update(cold=True,operation_counters=[{'before':process(10,100),'after':process(11,200)}]*32)
            self.write(f'1-64-{side}',value)
        identity={'executables':{n:{'sha256':'same-binary'} for n in ('before','after')}}
        (self.root/'manifest.json').write_text(json.dumps(identity))
        with self.assertRaisesRegex(ValueError,'mixed warm/cold'):
            paired.summarize([self.root],append=True)
        other=self.root/'cold';other.mkdir();(other/'manifest.json').write_text(json.dumps(identity))
        for path in self.root.glob('1-64-*'):path.rename(other/path.name)
        cold=paired.summarize([other],append=True)
        self.assertEqual(cold['append_mode'],'cold')
        self.assertIn('per-operation',cold['append_counter_scope'])
        for directories in ([self.root,other],[other,self.root]):
            with self.assertRaisesRegex(ValueError,'mixed warm/cold'):
                paired.summarize(directories,append=True)

    def test_cold_memory_maxima_include_operation_before_and_after_samples(self):
        self.test_append_endpoints_and_footprints()
        for side in ('before','after'):
            path=self.root/f'0-64-{side}.json';value=json.loads(path.read_text())
            before=process(rss=200*1024);before['smaps_rollup']+='Pss_Anon: 102400 kB\n'
            after=process(rss=300*1024);after['smaps_rollup']+='Pss_Anon: 153600 kB\n'
            value.update(cold=True,operation_counters=[{'before':before,'after':after}]*32)
            path.write_text(json.dumps(value))
        result=paired.summarize([self.root],append=True)['medians']['64']['before']
        self.assertEqual(result['max_sampled_rss_mib'],300)
        self.assertEqual(result['max_sampled_pss_mib'],300-128/1024)
        self.assertEqual(result['max_sampled_anon_pss_mib'],150)
        path=self.root/'0-64-before.json';value=json.loads(path.read_text())
        memory=value['operation_counters'][0]['before']
        memory['smaps_rollup']=memory['smaps_rollup'].replace('Rss: 204800 kB','Rss: 409600 kB')
        path.write_text(json.dumps(value))
        self.assertEqual(paired.append_sample(path)['max_sampled_rss_mib'],400)


if __name__ == '__main__':unittest.main()
