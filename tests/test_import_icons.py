"""Importer regression tests using the bundled QML source and SVGs."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from xml.etree import ElementTree


PROJECT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('import_icons', PROJECT / 'scripts/import-omapods-icons.py')
importer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(importer)


class ImportIconsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source = PROJECT / 'assets/AirPodsIcon.qml'
        cls.source_qml = cls.source.read_text(encoding='utf-8')
        # Use the shipped outlines to construct malformed inputs without extra artwork fixtures.
        cls.paths = {}
        for variant in ('pro', 'max', 'buds'):
            svg = ElementTree.parse(PROJECT / 'icons' / f'airpods-{variant}-symbolic.svg')
            cls.paths[variant] = svg.find('{http://www.w3.org/2000/svg}path').attrib['d']
        cls.bounds = {'pro': '0.25, 26.25, 37.5, 25.75',
                      'max': '0, 12, 34, 38', 'buds': '10.5, 26.5, 35, 25.5'}

    def qml(self, paths=None, bounds=None):
        paths = self.paths if paths is None else paths
        bounds = self.bounds if bounds is None else bounds
        return (f'property var ink: isPro ? [{bounds["pro"]}]\n'
                f': isMax ? [{bounds["max"]}] : [{bounds["buds"]}]\n'
                + '\n'.join(f'property string {variant}Path: "{path}"' for variant, path in paths.items()))

    def test_generated_svgs_match_all_bundled_artwork(self):
        generated = importer.render_icons(self.source_qml)
        expected_names = {f'airpods-{variant}-symbolic.svg' for variant in
                          ('pro', 'max', 'buds', 'pro-left', 'pro-right', 'buds-left', 'buds-right')}
        self.assertEqual(set(generated), expected_names)
        for name, text in generated.items():
            with self.subTest(name=name):
                self.assertEqual(text.encode('utf-8'), (PROJECT / 'icons' / name).read_bytes())

    def assert_preserves_outputs(self, qml, expected_error):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'AirPodsIcon.qml'
            source.write_text(qml, encoding='utf-8')
            destination = root / 'icons'
            destination.mkdir()
            existing = {name: f'previous {name}'.encode() for name in importer.render_icons(self.qml())}
            existing['airpods-case-symbolic.svg'] = b'original case artwork'
            for name, content in existing.items():
                (destination / name).write_bytes(content)
            with self.assertRaisesRegex(ValueError, expected_error):
                importer.import_icons(source, destination)
            self.assertEqual({path.name: path.read_bytes() for path in destination.iterdir()}, existing)

    def test_missing_empty_invalid_and_duplicate_paths_do_not_write(self):
        for variant in self.paths:
            for invalid_path in (None, '', 'not a path', 'M0 0<script>'):
                with self.subTest(variant=variant, path=invalid_path):
                    paths = dict(self.paths)
                    if invalid_path is None:
                        del paths[variant]
                    else:
                        paths[variant] = invalid_path
                    self.assert_preserves_outputs(self.qml(paths=paths), f'malformed {variant}Path')
            duplicate = self.qml() + f'\nproperty string {variant}Path: "M0 0Z"'
            self.assert_preserves_outputs(duplicate, f'malformed {variant}Path')

    def test_missing_and_invalid_bounds_do_not_write(self):
        self.assert_preserves_outputs(self.qml().replace('property var ink:', 'property var missing:'),
                                      'Unrecognized.*ink bounds')
        for variant in self.bounds:
            for invalid_bounds in ('0, 0, 1', '0, 0, 1, 1, 1', '0, 0, width, 1',
                                   'nan, 0, 1, 1', '0, inf, 1, 1', '0, 0, 1e999, 1',
                                   '0, 0, 0, 1', '0, 0, 1, 0', '0, 0, -1, 1', '0, 0, 1, -1'):
                with self.subTest(variant=variant, bounds=invalid_bounds):
                    bounds = {**self.bounds, variant: invalid_bounds}
                    self.assert_preserves_outputs(self.qml(bounds=bounds), f'{variant} ink bounds')

    def test_unattributed_source_does_not_write(self):
        self.assert_preserves_outputs(self.source_qml + '\n', 'differs from attributed revision')

    def test_validated_source_writes_only_expected_icons(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            destination = root / 'icons'
            destination.mkdir()
            (destination / 'airpods-case-symbolic.svg').write_text('original case', encoding='utf-8')
            generated = importer.import_icons(self.source, destination)
            for name, text in generated.items():
                self.assertEqual((destination / name).read_text(encoding='utf-8'), text)
                self.assertEqual((destination / name).read_bytes(), (PROJECT / 'icons' / name).read_bytes())
            self.assertEqual((destination / 'airpods-case-symbolic.svg').read_text(encoding='utf-8'), 'original case')


if __name__ == '__main__':
    unittest.main()
