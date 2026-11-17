"""Independent fixture checks for retained consumer-version and settlement evidence."""
import hashlib

NOTE = 'deterministic-note-payload-6d41'

def prefix(part, whole):
    if isinstance(whole, str):
        assert isinstance(part, str) and whole.startswith(part)
    elif isinstance(whole, dict):
        assert isinstance(part, dict) and part.keys() <= whole.keys()
        for key, value in part.items():
            prefix(value, whole[key])
    else:
        assert part == whole

def validate_pi_versions(sample, case, workspace=None):
    assert sample['observer_oracle_version'] == 2
    assert sample['observer_failure'] is None
    witnesses = sample['observer_versions']
    expected_count = {'short-small': 40, 'short-large': 96, 'tool': 2}.get(case, 1)
    completed = set()
    for witness in witnesses:
        assert witness['valid'] is True
        assert isinstance(witness['complete'], bool)
        ordinal = witness['ordinal']
        role = witness['role']
        assert role in ('assistant', 'toolResult', 'tool_output')
        if role == 'assistant':
            assert isinstance(ordinal, int) and not isinstance(ordinal, bool) and 0 <= ordinal < expected_count
            if case == 'tool' and ordinal == 0:
                assert isinstance(workspace, str)
                expected = {'type': 'toolCall', 'id': 'benchmark-read', 'name': 'read',
                            'arguments': {'path': workspace + '/note.txt'}}
                assert len(witness['blocks']) <= 1
                for block in witness['blocks']:
                    assert block['type'] == 'toolCall'
                    prefix(block, expected)
                    if witness['complete']:
                        assert block == expected
                if witness['complete']:
                    assert len(witness['blocks']) == 1
                    completed.add(ordinal)
                continue
            expected = ('L' * 32768 if case == 'large' else
                        ''.join(f'stream-delta-{i:04} ' for i in range(128)) if case == 'stream' else
                        'partial' if case == 'cancel' else NOTE if case == 'tool' else f'reply {ordinal:04}')
            if witness['complete']:
                assert case != 'cancel'
                completed.add(ordinal)
        else:
            assert case == 'tool' and witness['toolCallId'] == 'benchmark-read'
            expected = NOTE
        assert len(witness['blocks']) <= 1
        if witness['complete']:
            assert len(witness['blocks']) == 1
        for block in witness['blocks']:
            assert block['type'] == 'text'
            size = block['bytes']
            encoded = expected.encode()
            assert isinstance(size, int) and not isinstance(size, bool) and 0 <= size <= len(encoded)
            encoded[:size].decode('utf-8')  # no partial-codepoint prefix can stand in for valid text
            assert block['sha256'] == hashlib.sha256(encoded[:size]).hexdigest()
            if witness['complete']:
                assert size == len(encoded)
    assert completed == (set() if case == 'cancel' else set(range(expected_count)))
    explicit = sample['observer_explicit_run_ends']
    assert len(explicit) == len(set(explicit)) <= sample['history'] + sample['measured']

def validate_rust_settlements(sample, case):
    observed = sample['observer_validation']
    assert observed['oracle_version'] == 2
    explicit = observed['explicit_settlements']
    identities = [item['attempt_id'] for item in explicit]
    assert len(identities) == len(set(identities)) <= observed['completed_attempts']
    if identities:
        native = {item['snapshot']['identity']['attempt_id'] for item in sample['live_requests']}
        assert set(identities) <= native
    assert all(item['outcome']['type'] == ('cancelled' if case == 'cancel' else 'completed') for item in explicit)
    assert type(observed['checkpoint_terminal_sightings']) is int and observed['checkpoint_terminal_sightings'] >= 0
