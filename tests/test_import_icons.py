"""Keep the menu artwork consistent with the bundled QML source."""
import importlib.util
from pathlib import Path
import unittest


PROJECT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('import_icons', PROJECT / 'scripts/import-omapods-icons.py')
importer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(importer)


class ImportIconsTest(unittest.TestCase):
    def test_generated_svgs_match_all_bundled_artwork(self):
        generated = importer.render_icons((PROJECT / 'assets/AirPodsIcon.qml').read_text(encoding='utf-8'))
        expected_names = {f'airpods-{variant}-symbolic.svg' for variant in
                          ('pro', 'max', 'buds', 'pro-left', 'pro-right', 'buds-left', 'buds-right')}
        self.assertEqual(set(generated), expected_names)
        for name, text in generated.items():
            with self.subTest(name=name):
                self.assertEqual(text.encode('utf-8'), (PROJECT / 'icons' / name).read_bytes())


if __name__ == '__main__':
    unittest.main()
