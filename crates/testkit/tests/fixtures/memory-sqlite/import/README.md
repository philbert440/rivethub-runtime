# Postgres COPY import

`copy-cases.txt` is a handwritten, reviewed COPY fixture, not a pg_dump recording.
`expected.json` is its independent row-level SQLite v8 expectation, including every
column in every base table. JSON fields are compared as parsed values. The expected
little-endian float32 bytes represent `[0, 1]` and `[0.6, 0.8]`; timestamps are UTC
with the store's millisecond precision. Source row order deliberately puts children
before parents. The fixture includes both optional PostgreSQL 18 envelope additions;
the replay also exercises the PostgreSQL 16 envelope without them.

`seed.txt` populates the pre-0020 schema, using the vendored migrations 0001–0019.
To record it using a real installed pg_dump against an isolated sqlx test database:

```
SQLX_OFFLINE=true cargo test -p migrate --test record_import -- --ignored
```

The recording command requires `DATABASE_URL` and `pg_dump` on PATH. It writes
`pre-0020.pg.txt` here; the seed uses 1024-wide halfvec values, as required by 0015.
No database service is started or configured. Only the test's isolated database is
migrated and populated.

The real recording has not been produced in this environment: pg_dump is absent.
Do not describe the handwritten fixture as a recording or silently substitute it.

`skips.json` is the checked, empty list of out-of-slice import scenarios. All supplied
handwritten cases replay. A recording operation is explicitly invoked, not a replay
scenario. Existing recordings from other slices are not listed or skipped here.

The import command selects exactly the memory tables shared with Postgres. Other
Postgres tables (tasks, presets, chunks, the Graphile queue, migration bookkeeping)
are refused if included, since SQLite has no corresponding table. SQLite-local jobs,
metadata and the legacy embed queue are additionally supported in COPY input. The
`ros_*` names are the B5 compatibility schema names; v8 retains those tables. The
Postgres `content_hash` column is a derived chunk cache with no SQLite equivalent;
it is decoded but not stored. All other unknown columns are refused.

`pre-0020.expected.json` is the independently checked SQLite row expectation for
`seed.txt`. It checks every column, including the seed UUID relationships, tool
fields, UTC timestamps, taxonomy arrays, access counts and normalized 1024-wide
embeddings. The message and wiki vectors begin `[0.6, 0.8]`; the summary begins
`[0, 1]`; all remaining components are zero. SQLite-local tables have no imported
rows. The real recording replay also checks the report, receipt and search indexes.
