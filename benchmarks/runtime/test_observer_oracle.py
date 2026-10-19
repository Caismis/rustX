"""Independent consumer-witness negative checks; no provider/model API."""
import copy
import hashlib
import unittest
from observer_oracle import validate_pi_versions, validate_rust_settlements


def witness(text, complete=False):
    return {'ordinal':0,'role':'assistant','source':'message_end' if complete else 'snapshot_generation',
            'complete':complete,'valid':True,
            'blocks':[{'type':'text','bytes':len(text.encode()),'sha256':hashlib.sha256(text.encode()).hexdigest()}]}


def pi(versions):
    return {'observer_oracle_version':2,'observer_failure':None,'observer_versions':versions,
            'observer_explicit_run_ends':[7],'history':0,'measured':1}


class ConsumerWitnessTests(unittest.TestCase):
    def test_each_prior_version_is_independently_checked_even_if_final_is_correct(self):
        value=pi([witness('L'),witness('L'*32768,True)])
        validate_pi_versions(value,'large')
        changed=copy.deepcopy(value);changed['observer_versions'][0]=witness('WRONG')
        with self.assertRaises(AssertionError):validate_pi_versions(changed,'large')
        changed=copy.deepcopy(value);changed['observer_versions'][0]['valid']=False
        with self.assertRaises(AssertionError):validate_pi_versions(changed,'large')

    def test_sticky_failure_or_missing_final_cannot_be_hidden_by_a_snapshot(self):
        value=pi([witness('L'*32768,True)]);value['observer_failure']='prior invalid transient'
        with self.assertRaises(AssertionError):validate_pi_versions(value,'large')
        with self.assertRaises(AssertionError):validate_pi_versions(pi([witness('L')]),'large')
        with self.assertRaises(AssertionError):validate_pi_versions(pi([]),'large')

    def test_cancel_prefix_is_allowed_but_tail_or_canonical_partial_is_not(self):
        validate_pi_versions(pi([witness('part')]),'cancel')
        validate_pi_versions(pi([]),'cancel')
        for bad in [witness('partialmust not appear'),witness('partial',True)]:
            with self.assertRaises(AssertionError):validate_pi_versions(pi([bad]),'cancel')

    def test_duplicate_explicit_pi_run_end_is_distinct_from_snapshot_sightings(self):
        value=pi([]);value['observer_explicit_run_ends']=[];validate_pi_versions(value,'cancel')
        value['observer_explicit_run_ends']=[7,7]
        with self.assertRaises(AssertionError):validate_pi_versions(value,'cancel')

    def test_rust_explicit_settlements_are_not_deduplicated_checkpoint_sightings(self):
        value={'observer_validation':{'oracle_version':2,'completed_attempts':1,
               'checkpoint_terminal_sightings':3,'explicit_settlements':[
                   {'attempt_id':'native','outcome':{'type':'cancelled'}}]},
               'live_requests':[{'snapshot':{'identity':{'attempt_id':'native'}}}]}
        validate_rust_settlements(value,'cancel')
        changed=copy.deepcopy(value);changed['observer_validation']['explicit_settlements']*=2
        with self.assertRaises(AssertionError):validate_rust_settlements(changed,'cancel')
        changed=copy.deepcopy(value);changed['observer_validation']['explicit_settlements'][0]['attempt_id']='foreign'
        with self.assertRaises(AssertionError):validate_rust_settlements(changed,'cancel')
        changed=copy.deepcopy(value);changed['observer_validation']['explicit_settlements'][0]['outcome']['type']='completed'
        with self.assertRaises(AssertionError):validate_rust_settlements(changed,'cancel')


if __name__=='__main__':unittest.main()
