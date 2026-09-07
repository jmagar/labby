#!/usr/bin/env python3
"""Generate and verify provenance-bound migration rehearsal evidence."""
from __future__ import annotations
import argparse, hashlib, json, sqlite3
from pathlib import Path
from typing import Any

SCHEMA = "labby.multi-user-migration-rehearsal/v2"
MINIMUM_TABLES = {
    "labby": {"access_metadata", "organizations", "principals", "principal_links", "projects", "access_audit"},
    "depot": {"skills", "origins", "bundles", "jobs", "uploads", "artifacts"},
}

def fail(message: str) -> None: raise ValueError(message)

def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""): digest.update(chunk)
    return digest.hexdigest()

def encoded(value: Any) -> Any:
    return {"bytesSha256": hashlib.sha256(value).hexdigest(), "length": len(value)} if isinstance(value, bytes) else value

def inventory(path: Path, system: str) -> list[dict[str, Any]]:
    connection = sqlite3.connect(f"file:{path.resolve()}?mode=ro&immutable=1", uri=True)
    try:
        if connection.execute("PRAGMA quick_check").fetchone() != ("ok",): fail(f"{system} quick_check failed")
        if connection.execute("PRAGMA foreign_key_check").fetchall(): fail(f"{system} foreign_key_check failed")
        tables = [row[0] for row in connection.execute("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")]
        missing = MINIMUM_TABLES[system] - set(tables)
        if missing: fail(f"{system} database is missing required tables: {sorted(missing)}")
        result = []
        canonical = lambda value: json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
        for table in tables:
            quoted = '"' + table.replace('"', '""') + '"'
            columns = connection.execute(f"PRAGMA table_info({quoted})").fetchall()
            names = [column[1] for column in columns]
            primary = [column[1] for column in sorted(columns, key=lambda value: value[5]) if column[5]]
            order = primary or names
            order_sql = ",".join('"' + value.replace('"', '""') + '"' for value in order)
            rows = connection.execute(f"SELECT * FROM {quoted}" + (f" ORDER BY {order_sql}" if order_sql else "")).fetchall()
            logical = [[encoded(value) for value in row] for row in rows]
            ids = [[encoded(row[names.index(name)]) for name in primary] for row in rows] if primary else logical
            result.append({"table": table, "count": len(rows), "stableIdsSha256": hashlib.sha256(canonical(ids)).hexdigest(), "contentSha256": hashlib.sha256(canonical(logical)).hexdigest()})
        return result
    finally: connection.close()

def source(path: Path, system: str) -> dict[str, Any]:
    resolved = path.resolve(strict=True)
    return {"path": str(resolved), "sha256": sha256_file(resolved), "inventory": inventory(resolved, system)}

def generate(args: argparse.Namespace) -> dict[str, Any]:
    checkpoint, rollback = args.checkpoint.resolve(strict=True), args.rollback_checkpoint.resolve(strict=True)
    document = {"schemaVersion": SCHEMA, "operationId": args.operation_id, "sourceCommit": args.source_commit, "targetCommit": args.target_commit,
        "checkpoint": {"path": str(checkpoint), "sha256": sha256_file(checkpoint)}, "rollbackCheckpoint": {"path": str(rollback), "sha256": sha256_file(rollback)},
        "systems": {system: {stage: source(getattr(args, f"{system}_{stage}"), system) for stage in ("pre", "post")} for system in MINIMUM_TABLES}}
    validate(document); return document

def validate(document: Any) -> None:
    if not isinstance(document, dict) or document.get("schemaVersion") != SCHEMA: fail("unsupported rehearsal manifest schemaVersion")
    for field in ("operationId", "sourceCommit", "targetCommit"):
        if not isinstance(document.get(field), str) or not document[field].strip(): fail(f"{field} must be non-empty")
    checkpoint, rollback = document.get("checkpoint", {}), document.get("rollbackCheckpoint", {})
    if checkpoint.get("sha256") != rollback.get("sha256"): fail("rollback checkpoint must exactly match the pre-migration checkpoint")
    for label, value in (("checkpoint", checkpoint), ("rollbackCheckpoint", rollback)):
        path = Path(value.get("path", ""))
        if not path.is_absolute() or sha256_file(path) != value.get("sha256"): fail(f"{label} provenance does not match the actual file")
    systems = document.get("systems")
    if not isinstance(systems, dict) or set(systems) != set(MINIMUM_TABLES): fail("systems must contain exactly labby and depot")
    for system in sorted(MINIMUM_TABLES):
        stages = systems[system]
        if not isinstance(stages, dict) or set(stages) != {"pre", "post"}: fail(f"systems.{system} must contain pre and post")
        for stage in ("pre", "post"):
            claimed, path = stages[stage], Path(stages[stage].get("path", ""))
            if not path.is_absolute() or sha256_file(path) != claimed.get("sha256"): fail(f"systems.{system}.{stage} provenance does not match the actual store")
            if claimed.get("inventory") != inventory(path, system): fail(f"systems.{system}.{stage} inventory does not match the actual store")
        before = {row["table"]: row for row in stages["pre"]["inventory"]}; after = {row["table"]: row for row in stages["post"]["inventory"]}
        if set(before) - set(after): fail(f"systems.{system} lost tables: {sorted(set(before)-set(after))}")
        for table in sorted(set(before) & set(after) - {"access_metadata"}):
            if before[table] != after[table]: fail(f"systems.{system}.{table} changed durable inventory")

def main() -> None:
    root = argparse.ArgumentParser(); commands = root.add_subparsers(dest="command", required=True)
    check = commands.add_parser("verify"); check.add_argument("manifest", type=Path)
    create = commands.add_parser("generate")
    for flag in ("labby-pre", "labby-post", "depot-pre", "depot-post", "checkpoint", "rollback-checkpoint"): create.add_argument(f"--{flag}", type=Path, required=True)
    for flag in ("operation-id", "source-commit", "target-commit"): create.add_argument(f"--{flag}", required=True)
    create.add_argument("--output", type=Path, required=True); args = root.parse_args()
    try:
        if args.command == "verify": validate(json.loads(args.manifest.read_text()))
        else: args.output.write_text(json.dumps(generate(args), indent=2, sort_keys=True) + "\n")
    except (OSError, sqlite3.Error, json.JSONDecodeError, ValueError) as error: raise SystemExit(f"migration rehearsal rejected: {error}") from error

if __name__ == "__main__": main()
