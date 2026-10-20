#!/usr/bin/env python3
"""Publish small correctness evidence and an unexecuted performance protocol."""
from pathlib import Path
import datetime, hashlib, json, os, re, shutil, subprocess

root = Path('/workspace/pr485-study')
repo = root / 'metadata-ownership-candidate'
original = root / 'evidence/metadata-ownership-candidate-20261020'
fast = root / 'evidence/metadata-ownership-correctness-20261020'
out = root / 'evidence/metadata-ownership-delivery-20261020'
out.mkdir(exist_ok=False)

def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()

def sha(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()

def git(*args):
    return subprocess.check_output(['git', *args], cwd=repo, text=True).strip()

def save(name, value):
    (out / name).write_text(json.dumps(value, indent=2) + '\n')

head = git('rev-parse', 'HEAD')
assert head == '1dfdf7e53bf20e1869099578b8f41c434e275961'
assert not git('status', '--porcelain')
assert git('diff', '--name-only', '623b41b869a95c888eb04f907cbf607232699a2f', 'HEAD').splitlines() == [
    'src/runtime_client/projection.rs', 'src/runtime_client/projection/metadata_ownership_tests.rs']
final = json.loads((fast / 'trace-expectation/validation.json').read_text())
assert final['state'] == 'checks-passed' and all(run['exit'] == 0 for run in final['runs'])
for name, digest in final['source_sha256'].items():
    assert sha(repo / name) == digest
assert sha(final['binary']) == final['binary_sha256']
protected = json.loads((original / 'protected-identities-before.json').read_text())
verified = []
for path, expected in protected.items():
    actual = sha(path)
    assert actual == expected, path
    verified.append({'path': path, 'sha256': actual, 'bytes': Path(path).stat().st_size})
save('protected-identities-after.json', {'utc': now(), 'unchanged': True, 'files': verified})
env = os.environ | {'PATH': '/workspace/.cargo/bin:' + os.environ['PATH'], 'CARGO_HOME': '/workspace/.cargo',
                   'RUSTUP_HOME': '/workspace/.rustup', 'RUSTUP_TOOLCHAIN': '1.98.1'}
format_runs = []
for argv in [['cargo', 'fmt', '--all', '--', '--check'],
             ['rustfmt', '--edition', '2024', '--check', 'src/runtime_client/projection/metadata_ownership_tests.rs'],
             ['git', 'diff', '--check', '623b41b869a95c888eb04f907cbf607232699a2f', 'HEAD']]:
    result = subprocess.run(argv, cwd=repo, env=env, text=True, capture_output=True)
    format_runs.append({'argv': argv, 'exit': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr})
    assert result.returncode == 0, result.stderr
save('format-checks.json', format_runs)
base = json.loads((root / 'evidence/build-baseline/identity.json').read_text())
assert sha(base['binary']) == base['sha256']
assert all(sha(repo / name) == digest for name, digest in base['harness'].items())
ordinary = json.loads((original / 'test-orchestration.json').read_text())
assert sha(ordinary['binary']) == ordinary['sha256']
versions = []
for name, folder, commit, binary in [
    ('original-O3', original, 'da49cecc037d29f90aa48047c1f2441a4531c83c', ordinary),
    ('fixture-admission-O0', fast / 'fixture-admission', 'd6e850e3bb4d987b4fa439a09890e3aeba579755', json.loads((fast / 'fixture-admission/validation.json').read_text())),
    ('trace-expectation-O0', fast / 'trace-expectation', head, final)]:
    counts = {}
    for label in ['ownership', 'projection', 'checkpoint', 'settlement']:
        text = (folder / (label + '.stdout')).read_text()
        match = re.search(r'test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;', text)
        assert match, label
        counts[label] = {'passed': int(match[1]), 'failed': int(match[2]), 'ignored': int(match[3])}
    versions.append({'name': name, 'commit': commit, 'binary': binary['binary'],
                     'sha256': binary.get('binary_sha256', binary.get('sha256')), 'counts': counts})
assert sum(item['passed'] for item in versions[-1]['counts'].values()) == 110
checks = {'utc': now(), 'source_commit': head, 'base': base['head'], 'tree': git('rev-parse', 'HEAD^{tree}'),
          'src_tree': git('rev-parse', 'HEAD:src'), 'source_sha256': final['source_sha256'],
          'production_projection_blob': git('rev-parse', 'HEAD:src/runtime_client/projection.rs'),
          'production_file_identical_across_all_three_candidate_commits': True,
          'versions': versions, 'focused_total': {'passed': 110, 'failed': 0},
          'clippy': {'exit': final['runs'][-1]['exit'], 'scope': 'Direct clippy-driver check of default-feature root library-test crate with all Cargo-resolved original externs, -D warnings; not all-targets/all-features CI or FULL'},
          'full_ci_run': False, 'original_O3_build_exit': 0,
          'original_O3_build_finished': '2026-10-20T06:48:07.546334+00:00',
          'original_failures_retained': True, 'original_clippy_exit': 1,
          'original_fixture_problem': 'ContentRef.content_revision=u64::MAX violated its safe integer maximum; immutable Source revision is 1. It forced fitting before the expected path witnesses.',
          'independently_identified_failure_reproduced': 'After fixture correction the only failing case compared nested Trace revision 0 with outer revision 5. The final test captures the old nested checkpoint cut; production semantics unchanged.',
          'optimized_unit_not_a_performance_product': True,
          'protected_files_verified': len(verified), 'protected_bytes_verified': sum(item['bytes'] for item in verified),
          'minimum_available_bytes_during_corrected_build': final['runs'][0]['minimum_observed_available_bytes'],
          'available_bytes_now': os.statvfs(root).f_bavail * os.statvfs(root).f_frsize,
          'no_manual_cleanup': True, 'no_performance_runs': True, 'no_browser_replay': True}
save('validation-summary.json', checks)
script_names = ['compare-revisions.py', 'controller.py', 'compare-http.mjs', 'observer_oracle.py', 'summarize-comparisons.py', 'build-revision.py']
runtime_hashes = {name: sha(repo / 'benchmarks/runtime' / name) for name in script_names}
fixture = 'test-support/fake-provider/src/fake_provider/scenarios/runtime_benchmark.py'
mapping = [
    {'role': 'baseline-product', 'eligible_for_proposed_measurement': True, 'source_commit': base['head'],
     'src_tree': base['production_tree'], 'binary': base['binary'], 'sha256': base['sha256'], 'library_sha256': base['library_sha256'],
     'build': {'rustc': base['rustc'], 'opt_level': 3, 'debuginfo': 0, 'features': [], 'product_library_cfg_test': False, 'root_artifacts_fresh': False},
     'reuse_rule': 'Rehash immediately before each fresh same-machine baseline sample; do not reuse previous timing results.'},
    {'role': 'ownership-product', 'eligible_for_proposed_measurement': False, 'source_commit': head, 'src_tree': checks['src_tree'],
     'binary': None, 'sha256': None, 'status': 'No matching optimized conformance product exists yet. Must build exact reviewed commit with ordinary non-cfg(test) library and record fresh:false, full Cargo/rustc command, source tree, rlib and executable SHA before timing.'},
    {'role': 'original-candidate-O3-library-test', 'eligible_for_proposed_measurement': False, 'source_commit': versions[0]['commit'],
     'binary': ordinary['binary'], 'sha256': ordinary['sha256'], 'status': 'Wrong target: cfg(test) root library, old uncorrected test source. Production-file equality does not equate its build/source identity with the corrected conformance product.'},
    {'role': 'corrected-candidate-O0-library-test', 'eligible_for_proposed_measurement': False, 'source_commit': head,
     'binary': final['binary'], 'sha256': final['binary_sha256'], 'status': 'Correctness only: root O0/debuginfo0/lto-off; cached O3 externs. Never time as a production candidate.'},
    {'role': 'geometry/scoped-products', 'eligible_for_proposed_measurement': False,
     'source_commits': ['89d6b618eeba8afaf4b95af4406057330baa60a6', '31668f7a74b63446c538e00ed41ebfc42f0bc1a8'],
     'status': 'Contain other production changes and do not implement this isolated ownership candidate.'}]
save('binary-mapping.json', mapping)
allocated = {'live_short_large_product_max': 31576064, 'live_short_small_product_max': 8839168, 'no_live_short_large_product_max': 31539200}
estimated = 6 * sum(allocated.values())
protocol = {'status': 'PROPOSED ONLY; no performance launch or candidate product build in this delivery',
            'base_commit': base['head'], 'candidate_commit': head, 'binary_mapping': 'binary-mapping.json',
            'workload_script': 'benchmarks/runtime/compare-revisions.py', 'runtime_script_sha256': runtime_hashes,
            'fake_provider_sha256': sha(repo / fixture), 'fixture_harness_sha256': base['harness'],
            'source_tree_policy': 'Record full commit/src tree and raw source hashes including test-only differences. No binary identity inference solely from matching production-file text.',
            'blocks': [
                {'name': 'live-main', 'observer_mode': 'live-subscriber', 'cases': ['short-small', 'short-large'], 'pairs': 3, 'product_count': 12},
                {'name': 'late-consumer-control', 'observer_mode': 'no-live-subscriber', 'cases': ['short-large'], 'pairs': 3, 'product_count': 6}],
            'maximum_products': 18, 'pair_order': [['baseline', 'candidate'], ['candidate', 'baseline'], ['baseline', 'candidate']],
            'case_order_within_each_main_pair': ['short-small', 'short-large'],
            'per_product': {'fresh_process': True, 'fresh_native_store': True, 'fresh_sibling_local_HTTP_SSE_provider': True,
                            'same_persistent_filesystem_TMPDIR': str(root / 'tmp'), 'model_time': '2026-10-19 00:00:00',
                            'short_small': {'seed_turns': 8, 'measured_turns': 32, 'requests': 40},
                            'short_large': {'seed_turns': 64, 'measured_turns': 32, 'requests': 96}},
            'oracles': ['all complete ordered wire/native requests versus fixed oracle', 'exact late checkpoint/replay and all live subscriber versions/settlements', 'all live and reopened requests equal', 'all lifecycle endpoints through cleanup, product/provider exit 0'],
            'metrics': {'steady': 'Per-product measured-turn wall and CPU ticks, read/write accounting, sampled RSS/PSS; aggregate 32 measured turns without treating them as independent product repetitions.',
                        'lifecycle': 'Report initialization/seed, measured turns, after-turn verification, late snapshot, shutdown, driver drop, final writer close, reopen verification and cleanup separately. Whole product lifecycle is raw endpoint delta, not subtraction of medians.',
                        'provider': 'Sibling provider CPU/wall/I/O separately; product CPU excludes children, process I/O may include already-waited children. Provider teardown/controller total retained separately.',
                        'memory': 'Existing stat/smaps_rollup samples are sampled observations, not true peaks; no 1 MiB total-memory assertion.',
                        'statistics': 'All three per-case paired repetitions, individual ratios and medians/range; no precision or improvement acceptance claim from n=3; zero CPU tick denominator yields no invented ratio.'},
            'build_plan': {'executed': False, 'cwd': str(repo),
                           'argv': ['cargo', 'test', '--release', '--locked', '--offline', '--test', 'conformance', '--no-run', '--message-format=json', '-vv', '-j', '2'],
                           'environment': {'RUSTUP_TOOLCHAIN': '1.98.1', 'CARGO_TARGET_DIR': str(root / 'target'), 'CARGO_PROFILE_RELEASE_OPT_LEVEL': '3', 'CARGO_PROFILE_RELEASE_DEBUG': '0', 'CARGO_INCREMENTAL': '0', 'RUSTFLAGS': '-Copt-level=3 -Cdebuginfo=0', 'CARGO_BUILD_JOBS': '2', 'TMPDIR': str(root / 'tmp')},
                           'remove_environment_overrides': ['CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'LD_PRELOAD', 'CPUPROFILE', 'CARGO_PROFILE_RELEASE_LTO', 'CARGO_PROFILE_RELEASE_CODEGEN_UNITS'],
                           'identity_gate': 'Non-cfg(test) product library and conformance must be fresh:false and belong to this checkout. Preserve root lib/rmeta, conformance, bin helpers and required runtime shared libraries. No cargo clean; if cache identity is unexpectedly fresh, retain log and stop before timing.',
                           'upstream_builder_note': 'Existing build-revision.py calls cargo clean; do not execute it unchanged under the no-deletion constraint. Reuse its checked_artifacts identity validation on the standard non-clean Cargo invocation.'},
            'outer_orchestration_requirements': [
                'Published workload/controller/oracle bytes unchanged; register only baseline and ownership artifact maps in an independently reviewable outer adapter.',
                'Before each controller.run, verify/restore corresponding preserved CARGO_BIN_EXE helper executables/runtime libraries to their compiled paths and record hashes outside the measurement window.',
                'The existing run-native-cohort.py is hardcoded to the old three candidates and a 16 GiB reserve. It is not directly executable for this candidate at current capacity; any adapter/guard change must be explicit, separately reviewed, and retain the user-authorized 5 GiB hard floor.',
                'One product/provider pair at a time. No compilation, compression, profiling, browser, or other task timing concurrently.',
                'Preflight every product and continuously sample disk/process identity. Soft stop at 6 GiB, hard floor 5 GiB. Timeout each full product at 180 seconds and entire cohort at 30 minutes. On any failed oracle/exit/resource stop retain data and stop the cohort; no automatic retries.'],
            'resources': {'usable_bytes_at_plan': checks['available_bytes_now'], 'physical_memory_limit_bytes': int(Path('/sys/fs/cgroup/memory.max').read_text()),
                          'disk_hard_floor_bytes': 5 * 2**30, 'disk_soft_stop_bytes': 6 * 2**30,
                          'build_allocation_cap_bytes_proposed': 2 * 2**30, 'build_preflight_free_bytes_proposed': 8 * 2**30,
                          'run_evidence_cap_bytes': int(.75 * 2**30), 'active_store_tmp_cap_bytes': int(.25 * 2**30),
                          'run_preflight_free_bytes_proposed': int(7.25 * 2**30),
                          'combined_no_cleanup_capacity_envelope_bytes': int(9.25 * 2**30),
                          'observed_prior_allocations_only_not_timings': allocated, 'prior_layout_projection_for_18_products_bytes': estimated,
                          'no_delete_or_archive_loop': True,
                          'readiness': 'Current free space is below the proposed 8 GiB build entry gate; the O3 candidate artifact is absent. Do not start build/timing under this proposal until capacity/build bounds and the outer adapter are resolved. The original 16 GiB setup target remains unmet.'},
            'browser': 'Capacity-blocked; no replay, Docker storage changes, or browser launch in this protocol.',
            'pi': 'Not included; pinned 90d863f7c1183b965f099855715d6e88c7b8a52c remains deferred until Source pairing.'}
save('performance-protocol.json', protocol)
notes = '''Correctness results: 110 passed, 0 failed; direct root Clippy -D warnings exited 0. This is not FULL CI.
Source: 1dfdf7e53bf20e1869099578b8f41c434e275961, based on exact 623b41b869a95c888eb04f907cbf607232699a2f.
Production metadata move: da49cecc037d29f90aa48047c1f2441a4531c83c.
Test fixture admission correction: d6e850e3bb4d987b4fa439a09890e3aeba579755.
Nested legacy Trace expectation correction: 1dfdf7e53bf20e1869099578b8f41c434e275961.
Both test-only corrections were independently reviewed by coordinator before this delivery.

The original O3 library-test build took about 68 minutes. Cargo test filters operate when executing the binary, not when compiling: this --lib harness contains 4,199 tests. All 562 dependency artifacts were fresh; the root used O3/debuginfo0, no incremental cache, a single Cargo job, and default multiple codegen units. Observed workers progressed from opt cgu to lto cgu. Rustc documents default thin local LTO when LTO is unspecified, optimization enabled and multiple codegen units are used: https://doc.rust-lang.org/rustc/codegen-options/index.html#lto . No compiler self-profile was collected, so exact phase costs are not claimed.
Correctness iterations used the original Cargo-resolved O3 extern rlibs but directly compiled the root at O0/debuginfo0, explicit lto=off, release debug-assertion/overflow behavior, 256 codegen units and a separate /tmp incremental directory, with two-CPU affinity. The original O3 binary and both O0 binaries remain preserved. No O0 timing is proposed.

The original O3 run exposed an invalid test fixture before it reached the reviewed Trace assertion. The intermediate fixture correction retained that assertion and reproduced revision 0 versus 5. All raw failure logs are retained. The final run passed all eight branch/error equivalence tests plus 102 existing regressions, with full event/structural-byte oracles and explicit cursor/Trace checks.

The only eligible existing performance executable is the exact 623 baseline conformance product. No isolated ownership conformance O3 product exists yet. The old O3 unit harness has cfg(test) and the old test source; production-file text equality alone does not allow relabeling its source/build identity. Geometry/scoped contain other changes and are excluded.
The proposed native-only 18-product protocol is not executed. The original workload/controller/oracle remain fixed. Ordinary turns, late snapshot/verification, reopen/close and whole lifecycle must be reported separately. Sampled memory and process writes are not allocation totals or physical disk traffic. Existing wrapper/build scripts have explicit integration constraints recorded in performance-protocol.json; they must not be silently bypassed.
No browser replay, performance product, Pi run, Docker storage change, main/PR #485 modification, merge, manual deletion, or archive loop occurred in this work.
'''
(out / 'READ-ME.txt').write_text(notes)
for name in ['build-orchestration.json', 'build-final-validated.json', 'guard-extension.json', 'test-orchestration.json', 'lint-orchestration.json', 'cargo.stdout.jsonl', 'cargo.stderr',
             'ownership.stdout', 'ownership.stderr', 'projection.stdout', 'projection.stderr', 'checkpoint.stdout', 'checkpoint.stderr', 'settlement.stdout', 'settlement.stderr', 'clippy.stdout.jsonl', 'clippy.stderr']:
    target = out / 'original-O3' / name
    target.parent.mkdir(exist_ok=True)
    shutil.copy2(original / name, target)
for label in ['fixture-admission', 'trace-expectation']:
    target = out / label
    target.mkdir()
    for name in ['validation.json', 'build.stdout', 'build.stderr', 'ownership.stdout', 'ownership.stderr', 'projection.stdout', 'projection.stderr', 'checkpoint.stdout', 'checkpoint.stderr', 'settlement.stdout', 'settlement.stderr', 'clippy.stdout', 'clippy.stderr']:
        source = fast / label / name
        if source.exists():
            shutil.copy2(source, target / name)
scripts = out / 'scripts'
scripts.mkdir()
for name in ['build-metadata-ownership-tests.py', 'extend-existing-metadata-build-monitor.py', 'test-metadata-ownership.py', 'lint-metadata-ownership.py', 'continue-metadata-validation.py', 'check-metadata-ownership-fast.py', 'deliver-metadata-validation.py']:
    shutil.copy2(root / name, scripts / name)
for commit in ['da49cecc037d29f90aa48047c1f2441a4531c83c', 'd6e850e3bb4d987b4fa439a09890e3aeba579755', head]:
    (out / (commit[:8] + '.patch')).write_bytes(subprocess.check_output(['git', 'format-patch', '-1', commit, '--stdout'], cwd=repo))
(out / 'combined-baseline-to-candidate.patch').write_bytes(subprocess.check_output(['git', 'diff', '--binary', base['head'], head], cwd=repo))
files = [{'path': str(path.relative_to(out)), 'bytes': path.stat().st_size, 'sha256': sha(path)} for path in sorted(out.rglob('*')) if path.is_file()]
save('raw-files-manifest.json', {'files': files, 'total_bytes': sum(item['bytes'] for item in files), 'archives_created': 0, 'binary_bytes_not_copied_to_delivery': True})
state = json.loads((root / 'STATUS.json').read_text())
state.update(updated_utc=now(), status='Metadata ownership candidate 1dfdf: 110 focused correctness tests and direct root Clippy passed; protocol only, O3 performance candidate absent.')
state['active_metadata_candidate'] = {'state': 'correctness-passed-protocol-delivered', 'source_commit': head, 'branch': git('branch', '--show-current'),
                                      'validation_summary': str(out / 'validation-summary.json'), 'performance_protocol': str(out / 'performance-protocol.json'), 'performance_started': False}
(root / 'STATUS.json').write_text(json.dumps(state, indent=2) + '\n')
print(json.dumps({'delivery': str(out), 'files': len(files), 'bytes': sum(item['bytes'] for item in files), 'source_commit': head, 'focused_passed': 110, 'protected_verified': len(verified), 'available_bytes': checks['available_bytes_now']}), flush=True)
