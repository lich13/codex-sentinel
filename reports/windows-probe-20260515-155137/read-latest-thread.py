import json, sqlite3, sys
db = sys.argv[1]
conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
conn.row_factory = sqlite3.Row
row = conn.execute("SELECT id, title, cwd, updated_at, rollout_path FROM threads WHERE coalesce(archived, 0) = 0 ORDER BY updated_at DESC LIMIT 1").fetchone()
print(json.dumps(dict(row) if row else None, ensure_ascii=False))
