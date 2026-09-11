"""Packed booleans: bit order, sample joins, preservation, and strict refusal."""
import copy
import tempfile
import textwrap
import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np
import uproot

from rowver import rvr
from rowver import Cart
from support import snapshot


class BooleanBitsTests(unittest.TestCase):
    def test_untyped_empty_chunk_does_not_widen_boolean_column(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            with uproot.recreate(root/'s.root') as f:
                f.mktree('Events', {'x': 'int32'})
                f['Events'].extend({'x': np.arange(6, dtype=np.int32)})
            (root/'cart.py').write_text(textwrap.dedent('''
                import numpy as np
                from rowver import Cart
                CART = Cart(fileset={'s': {'files': {'s.root': 'Events'}, 'metadata': {'weights': None}}},
                            columns={'flag': lambda e: np.array([]) if len(e) == 0 else np.asarray(e.x % 2 == 0)},
                            keep=lambda e: np.asarray(e.x < 3))
            '''))
            for workers in (None, 2):
                with self.subTest(workers=workers):
                    out = Cart.load(root).build(root/f'{workers}.rvr', step_size=3, workers=workers)
                    meta, buf = rvr.read(out['file'])
                    self.assertEqual(meta['data']['cols'][0]['kind'], 'b1')
                    np.testing.assert_array_equal(rvr.columns(meta, buf)['flag'], [True, False, True])

    def test_empty_root_sample_does_not_widen_boolean_values(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for name, n in [('populated', 9), ('empty', 0)]:
                with uproot.recreate(root/f'{name}.root') as f:
                    f.mktree('Events', {'flag': 'bool'})
                    if n: f['Events'].extend({'flag': np.arange(n)%2 == 0})
            (root/'cart.py').write_text(textwrap.dedent('''
                import numpy as np
                from rowver import Cart
                CART = Cart(fileset={s: {'files': {s+'.root': 'Events'}, 'metadata': {'weights': None}}
                                     for s in ('populated', 'empty')},
                            columns={'flag': 'flag'}, keep=lambda e: np.ones(len(e), dtype=bool))
            '''))
            for workers in (None, 2):
                with self.subTest(workers=workers), patch.object(rvr, 'MAX_DATA_BYTES', 40):
                    out = Cart.load(root).build(root/f'{workers}.rvr', workers=workers)
                    meta, buf = rvr.read(out['file'])
                    self.assertEqual(meta['data']['cols'][0]['kind'], 'b1')
                    self.assertEqual(len(buf), 38)
                    np.testing.assert_array_equal(rvr.columns(meta, buf)['flag'], np.arange(9)%2 == 0)

    def test_serial_and_parallel_presser_size_limit_uses_bits(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            paths = []
            for index, n in enumerate((33, 32)):
                p = root/f'{index}.root'
                with uproot.recreate(p) as f:
                    f.mktree('Events', {'flag': 'bool'})
                    f['Events'].extend({'flag': np.arange(n) % 3 == 1})
                paths.append(str(p))
            (root/'cart.py').write_text(textwrap.dedent(f'''
                import numpy as np
                from rowver import Cart
                CART = Cart(fileset={{'sample': {{'files': {{p: 'Events' for p in {paths!r}}},
                                                  'metadata': {{'weights': None}}}}}},
                            columns={{'flag': 'flag'}},
                            keep=lambda e: np.ones(len(e), dtype=bool))
            '''))
            expected = np.concatenate([np.arange(n)%3 == 1 for n in (33, 32)])
            for workers in (None, 2):
                with self.subTest(workers=workers), patch.object(rvr, 'MAX_DATA_BYTES', 300):
                    # Bits + sample IDs need 269 B, while int32 flags need 520 B.
                    out = Cart.load(root).build(root/f'{workers}.rvr', step_size=7, workers=workers)
                    meta, buf = rvr.read(out['file'])
                    self.assertEqual(len(buf), 269)
                    np.testing.assert_array_equal(rvr.columns(meta, buf)['flag'], expected)

    def test_size_estimate_never_refuses_what_pack_accepts(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            with uproot.recreate(root/'s.root') as f:
                f.mktree('Events', {'x': 'int64'})
                f['Events'].extend({'x': np.array([2**40, 1, 2, 3, 4, 5], dtype=np.int64)})
            (root/'cart.py').write_text(textwrap.dedent('''
                import numpy as np
                from rowver import Cart
                CART = Cart(fileset={'s': {'files': {'s.root': 'Events'}, 'metadata': {'weights': None}}},
                            columns={'c': lambda e: np.asarray(e.x) if e.x[0] > 3 else np.asarray(e.x, dtype=float)},
                            keep=lambda e: np.ones(len(e), dtype=bool))
            '''))
            for workers in (None, 2):
                with self.subTest(workers=workers):
                    out = Cart.load(root).build(root/f'{workers}.rvr', step_size=3, workers=workers)
                    meta, buf = rvr.read(out['file'])
                    self.assertEqual(meta['data']['cols'][0]['kind'], 'f8')
                    np.testing.assert_array_equal(rvr.columns(meta, buf)['c'], [2**40, 1, 2, 3, 4, 5])

    def test_bits_and_source_order_across_sample_and_byte_boundaries(self):
        for n in (0, 1, 7, 8, 9, 15, 16, 17, 257):
            with self.subTest(rows=n):
                flags = np.arange(n) % 3 == 1
                split = n // 2
                meta, buf = rvr.pack({
                    name: {'flag': flags[lo:hi], 'i': np.arange(lo, hi, dtype=np.int32),
                           'x': np.arange(lo, hi, dtype=np.float64),
                           'true': np.ones(hi-lo, dtype=bool),
                           'false': np.zeros(hi-lo, dtype=bool)}
                    for name, lo, hi in [('a', 0, split), ('b', split, n)]
                }, weighted={'a': False, 'b': False}, made=snapshot('a', 'b'))
                self.assertEqual(meta['rowver'], rvr.ROWVER)
                self.assertEqual([c['name'] for c in meta['data']['cols']],
                                 ['flag', 'i', 'x', 'true', 'false'])
                self.assertEqual(len(buf), 16*n + 3*((n+7)//8))
                col = meta['data']['cols'][0]
                # Independent bit assembly, with sample boundaries inside bytes.
                expected = bytearray((n+7)//8)
                for i, flag in enumerate(flags):
                    expected[i//8] |= int(flag) << (i%8)
                self.assertEqual(buf[col['at']:col['at']+len(expected)], expected)
                with tempfile.TemporaryDirectory() as tmp:
                    path = Path(tmp)/'bits.rvr'
                    with patch('numpy.unpackbits', side_effect=AssertionError('eager unpack')):
                        rvr.write(path, meta, buf)
                        back, payload = rvr.read(path)
                    self.assertEqual(payload, buf)
                    arrays = rvr.columns(back, payload)
                    self.assertEqual(arrays['flag'].dtype, np.dtype(bool))
                    np.testing.assert_array_equal(arrays['flag'], flags)
                    _, by_sample = rvr.read_by_sample(path)
                    np.testing.assert_array_equal(by_sample['b']['flag'], flags[split:])

    def test_numeric_zero_one_is_not_retyped_and_mixed_samples_widen(self):
        for other, expected in ((np.array([0, 1], dtype=np.int8), 'i4'),
                                (np.array([0., 1.]), 'f8')):
            meta, buf = rvr.pack({'a': {'x': np.array([True, False])},
                                 'b': {'x': other}}, weighted={'a': False, 'b': False},
                                made=snapshot('a', 'b'))
            self.assertEqual(meta['data']['cols'][0]['kind'], expected)
            np.testing.assert_array_equal(rvr.columns(meta, buf)['x'], [1, 0, 0, 1])

    def test_bad_bits_and_geometry_are_refused(self):
        m, b = rvr.pack({'s': {'flag': np.array([True]*9)}}, weighted={'s': False}, made=snapshot('s'))
        at = m['data']['cols'][0]['at']
        corrupt = bytearray(b); corrupt[at+1] |= 128
        with self.assertRaisesRegex(rvr.FormatError, 'tail bits'):
            rvr.check(m, corrupt)
        for change in ('version', 'offset', 'lim', 'vals', 'size'):
            bad = copy.deepcopy(m)
            c = bad['data']['cols'][0]
            if change == 'version': bad['rowver'] = 7
            if change == 'offset': c['at'] = 0
            if change == 'lim': c['lim'] = [0, 1]
            if change == 'vals': c['vals'] = [0, 1]
            if change == 'size': bad['data']['bytes'] += 1
            with self.subTest(change=change), self.assertRaises(rvr.FormatError):
                rvr.check(bad, b)

    def test_save_keeps_bits_and_unknown_metadata(self):
        m, b = rvr.pack({'s': {'flag': np.array([True, False, True])}}, made=snapshot('s'),
                        weighted={'s': False}, labels={'flag': {0: 'no', 1: 'yes'}})
        self.assertEqual(m['data']['cols'][0]['names'], {'0': 'no', '1': 'yes'})
        m['private.extension'] = {'keep': ['exactly']}
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)/'save.rvr'
            rvr.write(path, m, b)
            back, buf = rvr.read(path)
            back['made']['title'] = 'Edited'
            back['made']['sha256'] = rvr.made_digest(back['made'])
            rvr.write(path, back, buf)
            final, saved = rvr.read(path)
        self.assertEqual(saved, b)
        self.assertEqual(final['data'], m['data'])
        self.assertEqual(final['made']['title'], 'Edited')
        self.assertEqual(final['private.extension'], m['private.extension'])


if __name__ == '__main__':
    unittest.main()
