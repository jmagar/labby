import argparse, importlib.util, shutil, sqlite3, tempfile, unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("validate-multi-user-migration-rehearsal.py")
SPEC = importlib.util.spec_from_file_location("migration_rehearsal", SCRIPT); assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC); SPEC.loader.exec_module(MODULE)

class MigrationRehearsalTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(); self.root = Path(self.temp.name); self.paths = {}
        for system in MODULE.MINIMUM_TABLES:
            for stage in ("pre", "post"):
                path = self.root / f"{system}-{stage}.db"; connection = sqlite3.connect(path)
                for table in sorted(MODULE.MINIMUM_TABLES[system]):
                    connection.execute(f'CREATE TABLE "{table}" (id TEXT PRIMARY KEY, value TEXT)')
                    connection.execute(f'INSERT INTO "{table}" VALUES (?, ?)', (f"{table}-1", "preserved"))
                connection.commit(); connection.close(); self.paths[f"{system}_{stage}"] = path
        self.checkpoint = self.root / "checkpoint.db"; self.rollback = self.root / "rollback.db"
        shutil.copy2(self.paths["labby_pre"], self.checkpoint); shutil.copy2(self.checkpoint, self.rollback)

    def tearDown(self): self.temp.cleanup()

    def args(self):
        return argparse.Namespace(**self.paths, checkpoint=self.checkpoint, rollback_checkpoint=self.rollback,
            operation_id="ci-rehearsal", source_commit="source", target_commit="target")

    def test_generator_and_verifier_bind_actual_stores(self): MODULE.validate(MODULE.generate(self.args()))

    def test_fabricated_inventory_is_rejected(self):
        manifest = MODULE.generate(self.args()); manifest["systems"]["labby"]["pre"]["inventory"][0]["count"] += 1
        with self.assertRaisesRegex(ValueError, "inventory does not match"): MODULE.validate(manifest)

    def test_changed_checkpoint_is_rejected(self):
        manifest = MODULE.generate(self.args()); self.checkpoint.write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "provenance"): MODULE.validate(manifest)

    def test_durable_content_drift_is_rejected(self):
        connection = sqlite3.connect(self.paths["depot_post"]); connection.execute("UPDATE skills SET value='drift'"); connection.commit(); connection.close()
        with self.assertRaisesRegex(ValueError, "changed durable inventory"): MODULE.generate(self.args())

    def test_missing_actual_table_is_rejected(self):
        connection = sqlite3.connect(self.paths["labby_post"]); connection.execute("DROP TABLE projects"); connection.commit(); connection.close()
        with self.assertRaisesRegex(ValueError, "missing required tables"): MODULE.generate(self.args())

if __name__ == "__main__": unittest.main()
