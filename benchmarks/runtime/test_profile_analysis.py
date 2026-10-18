"""Deterministic executable-identity and PIE-address regression fixtures."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT=Path(__file__).parent/'profiling/analyze-heap.py'
class HeapAnalysisTests(unittest.TestCase):
    def run_fixture(self, mapping_name, binary_name='renamed-engine', offset_zero=True):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);binary=root/binary_name;binary.touch()
            symbols=root/'symbols';symbols.write_text('0000000000002100 t rustx::allocation_site\n')
            profiles=[]
            for count in [1,4]:
                p=root/f'{count}.heap'
                maps=(f'100000-101000 r--p 00000000 00:00 1 {root/mapping_name}\n' if offset_zero else '')
                # Executable segment file offset 0x1000 differs from vaddr 0x2000.
                maps+=f'102000-103000 r-xp 00001000 00:00 1 {root/mapping_name}\n'
                p.write_text(f'heap profile: 0: 0 [{count}: {count*64}] @ heapprofile\n 0: 0 [{count}: {count*64}] @ 0x102110\nMAPPED_LIBRARIES:\n'+maps)
                profiles.append(p)
            return subprocess.run([sys.executable,str(SCRIPT),'--binary',str(binary),str(symbols),*map(str,profiles)],capture_output=True,text=True)

    def test_renamed_binary_and_segment_gap(self):
        result=self.run_fixture('renamed-engine')
        self.assertEqual(result.returncode,0,result.stderr)
        parsed=json.loads(result.stdout)
        self.assertEqual(parsed['bytes'],[['rustx::allocation_site',192]])
        self.assertEqual(parsed['counts'],[['rustx::allocation_site',3]])

    def test_missing_binary_mapping_has_actionable_error(self):
        result=self.run_fixture('different-engine')
        self.assertNotEqual(result.returncode,0)
        self.assertIn('no mappings for executable',result.stderr)
        self.assertIn('renamed-engine',result.stderr)
        self.assertNotIn('Traceback',result.stderr)

    def test_missing_zero_offset_rejects_ambiguous_load_base(self):
        result=self.run_fixture('renamed-engine',offset_zero=False)
        self.assertNotEqual(result.returncode,0)
        self.assertIn('cannot establish PIE load base',result.stderr)

if __name__=='__main__':unittest.main()
