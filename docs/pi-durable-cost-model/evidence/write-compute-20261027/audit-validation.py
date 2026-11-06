#!/usr/bin/env python3
"""Audit saved correctness and Clippy output; never execute test/build commands."""
import collections
import hashlib
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parent
CHECK = ROOT/'check'
REPO = Path('/home/capi/.codex/worktrees/pr485-pi-performance/rustX')
REVISION = '2800be25a650305d5daf9f543ffbf7d8f62ea79b'
SOURCE_TREE = 'a86302ed48b415de91cac782bd156dc935b8e517'

def load(path):
    return json.loads(path.read_text())

def digest(path):
    with path.open('rb') as handle:
        return hashlib.file_digest(handle, 'sha256').hexdigest()

def git(*args):
    return subprocess.check_output(['git', '-C', str(REPO), *args], text=True).strip()

assert git('rev-parse', REVISION+':src') == SOURCE_TREE
commands = load(CHECK/'check-build-commands.json')
by_label = {row['label']: row for row in commands}
assert len(commands) == len(by_label) == 10
assert all(row['status'] == 'complete' for row in commands)
assert all(a['end_utc'] <= b['start_utc'] for a,b in zip(commands, commands[1:]))
assert all(row['exit'] == (101 if row['label'] == 'clippy' else 0) for row in commands)
build_command = by_label['tests-build']['command']
assert build_command == ['cargo','test','--release','--config','profile.release.package.rustx.opt-level=0','--locked','--all-features','--lib','--test','durable','--test','process','--no-run','--message-format=json']
build_rows = [json.loads(line) for line in (CHECK/'tests-build.stdout').read_text().splitlines() if line.startswith('{')]
assert [row for row in build_rows if row.get('reason') == 'build-finished'] == [{'reason':'build-finished','success':True}]
artifacts = [row for row in build_rows if row.get('reason') == 'compiler-artifact']
root_artifacts = [row for row in artifacts if row['target']['src_path'].startswith(str(REPO)+'/')]
assert len(root_artifacts) == 7
assert all(row['profile']['opt_level'] == '0' and not row['fresh'] for row in root_artifacts)
assert all(row['features'] == ['issue-387-profile','mcp-fixture'] for row in root_artifacts)
assert all(not row['profile']['debug_assertions'] and not row['profile']['overflow_checks'] for row in root_artifacts)
executables = {row['executable'] for row in root_artifacts if row.get('executable')}
dependencies = collections.Counter((row['profile']['opt_level'],tuple(row['target']['kind']),row['profile']['test']) for row in artifacts if row not in root_artifacts)

names = collections.defaultdict(list)
scoped_names = collections.defaultdict(list)
groups = []
for label in ['tests-sqlite','tests-client','tests-conversation-runtime','tests-read-layout','tests-content','tests-durable','tests-process']:
    command = by_label[label]['command']
    assert command[0] in executables and command[-1] == '--test-threads=2'
    output = (CHECK/(label+'.stdout')).read_text()
    results = re.findall(r'^test (\S+) \.\.\. (ok|ignored|FAILED)', output, re.M)
    assert len({name for name,_ in results}) == len(results)
    summary = re.findall(r'^test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out;',output,re.M)
    assert len(summary) == 1
    status, passed, failed, ignored, measured, filtered = summary[0]
    passed, failed, ignored, measured, filtered = map(int,[passed, failed, ignored, measured, filtered])
    assert status == 'ok' and failed == measured == 0
    assert collections.Counter(result for _,result in results) == collections.Counter({'ok':passed,'ignored':ignored})
    assert int(re.search(r'^running (\d+) tests$',output,re.M)[1]) == passed+ignored
    selector = None if label in ['tests-durable','tests-process'] else command[1]
    if selector:
        assert command[2:4] == ['--skip','::scale::']
        assert all(selector in name and '::scale::' not in name for name,_ in results)
    else:
        assert filtered == 0
    for name,result in results:
        names[name].append({'group':label,'status':result})
        scoped_names[(command[0],name)].append(label)
    groups.append({'group':label,'command':command,'selector':selector,'passed':passed,
        'ignored':ignored,'failed':failed,'filtered_out':filtered,
        'namespace_counts':dict(collections.Counter(name.split('::')[0] for name,_ in results)),
        'stdout_sha256':digest(CHECK/(label+'.stdout')),
        'stderr_sha256':digest(CHECK/(label+'.stderr'))})
duplicates = {name:rows for name,rows in names.items() if len(rows)>1}
assert not duplicates and all(len(rows)==1 for rows in scoped_names.values())
assert sum(row['passed'] for row in groups) == 1323
assert sum(row['ignored'] for row in groups) == 1
ignored = [name for name,rows in names.items() if rows[0]['status']=='ignored']
assert ignored == ['runtime_client::host::tests::t19_native_publication_encoding_profile']
critical = {name:rows for name,rows in names.items() if any(part in name for part in ['::native_length_tests::','::metadata_ownership_tests::','::final_settlement_tests::final_model_'])}
assert len(critical) == 20 and all(rows[0]['status']=='ok' for rows in critical.values())

initial = (CHECK/'clippy.stderr').read_text()
assert initial.count('error: item in documentation is missing backticks') == 2
assert 'final_settlement.rs:30:' in initial and 'final_settlement.rs:32:' in initial
final = load(ROOT/'clippy-final-command.json')
assert final['exit'] == 0 and final['command'] == by_label['clippy']['command']
assert final['command'] == ['cargo','clippy','--release','--locked','--all-targets','--all-features','--','-D','warnings']
assert by_label['clippy']['end_utc'] <= final['start_utc'] <= final['end_utc']
final_stderr = (ROOT/'clippy-final.stderr').read_text()
assert 'Finished `release` profile [optimized]' in final_stderr
assert 'error:' not in final_stderr and 'warning:' not in final_stderr
source = git('show', REVISION+':src/durable/sqlite/final_settlement.rs')
assert 'nor a live `ContentRef` may escape' in source
assert 'by `canonical_publication_tx`.' in source

report = {'accepted':True,'audit_method':'Read saved commands, Cargo artifact JSON and exact test names; no build/test/product run.',
    'source_revision':REVISION,'source_tree':SOURCE_TREE,
    'build':{'command':build_command,'exit':0,'root_opt_level':0,
        'root_debug_assertions':False,'root_overflow_checks':False,
        'features':['issue-387-profile','mcp-fixture'],
        'scope':'Correctness: root package O0 with release-profile dependencies, all features. Not the ordinary O3 performance build. Selected lib groups plus complete durable/process integration executables, not all crate tests.',
        'root_artifacts':[{'name':row['target']['name'],'kind':row['target']['kind'],'test':row['profile']['test'],'executable':row.get('executable'),'fresh':row['fresh']} for row in root_artifacts],
        'dependency_artifact_profile_counts':[{'opt_level':level,'kind':list(kind),'test':test,'artifact_rows':count} for (level,kind,test),count in sorted(dependencies.items())],
        'dependency_scope_note':'418 artifact rows use O3; 144 use O0 (54 custom-build, 29 proc-macro, 61 library artifacts in the dependency graph). Do not describe every dependency artifact as O3.',
        'stdout_sha256':digest(CHECK/'tests-build.stdout'),'stderr_sha256':digest(CHECK/'tests-build.stderr')},
    'groups':groups,'totals':{'passed_executions':1323,'failed':0,'ignored':1,'unique_test_names':1324,'unique_passed_tests':1323,'duplicate_names':0,'same_executable_repeats':0},
    'duplicates':duplicates,'ignored_tests':[{'name':ignored[0],'reason':'measurement only; run explicitly with --ignored --nocapture'}],
    'substring_filter_scope':'Libtest substring filters, not anchored module selectors. runtime_client:: includes 417 runtime_client + 6 boundary_suites + 110 scripted_suites names; exact names across all seven logs have no overlap. ::scale:: skipped in five filtered lib groups.',
    'critical_relevant_tests':critical,
    'format':{'command':by_label['fmt']['command'],'exit':0,'scope':'Ran before the later two doc-comment backtick fixes.'},
    'clippy':{'initial_exit':101,'initial_errors':2,'initial_lint':'clippy::doc_markdown',
        'initial_locations':['src/durable/sqlite/final_settlement.rs:30','src/durable/sqlite/final_settlement.rs:32'],
        'fixes':['ContentRef wrapped in backticks','canonical_publication_tx wrapped in backticks'],
        'final_command':final['command'],'final_exit':0,'final_warnings':0,'final_stderr_sha256':digest(ROOT/'clippy-final.stderr'),
        'scope_record':final['scope'],'scope_note':'Initial failing output retained. Committed documentation lines match both fixes; command record says no executable-source change between correctness run and rerun.'},
    'limitations':['No per-build source-tree hash was frozen before the pre-commit correctness run; source association also uses the recorded doc-only follow-up scope and reviewed committed diff.','Passing selected checks does not establish every feature/configuration or ordinary O3 test behavior.']}
(ROOT/'validation-audit.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps({'accepted':True,'totals':report['totals'],'groups':[{k:r[k] for k in ['group','passed','ignored','failed']} for r in groups],'clippy_final_exit':0},indent=2))
