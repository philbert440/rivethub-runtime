# Deviations from the recordings

Each entry names the recorded exchange that the wire types do not reproduce and why. Skipped cases are skipped by
id in `roundtrip.rs`; the one field-level delta below is applied where named. Nothing else is skipped.

## `providers/vllm/*/requests.json` — request fields the runtime omits

Every recorded vLLM request body carries `"temperature":0.7,"top_p":0.95`, a `tool_choice:"auto"`
and `max_tokens:4096`: the TypeScript provider sent the sampling pair hard-coded, `tool_choice`
with every tools-bearing body, and `max_tokens` from its own default. The runtime sends NONE of
these to a model server — sampling and tuning live on the model server, `tool_choice` is left at
the server default (`auto` when `tools` are present), and `max_tokens` is sent only when the
provider config sets it (CONTRACT decision 11, boundary B8) — so `openai::ChatCompletionRequest`
has no `temperature`/`top_p`, and the roundtrip removes exactly those four members from the
recorded body before comparing. The table is typed and shared with the providers replay:
`testkit::provider::B8_OMITTED_REQUEST_FIELDS` pairs each field with a `WireDelta` reason
(`SamplingOnModelServer`, `OutputLimitOnModelServer`, `ServerDefaultToolChoice`).

The recordings live once under `crates/testkit/tests/fixtures/providers/vllm/` and this test reads
them through `testkit::recording`; the `providers/ollama/` and `providers/llama-server/` recordings
and their wire types were removed with the provider types they served, so the vLLM recordings are
the provider traffic that remains.

## `gateway/http.json` — `event-reserialize`, `gateway/ws.json` — `post-reserialize`

The producer sent `"ts":1.7600000045e12` on a `session.start` event: a JavaScript float spelling of the epoch-ms
value. Epoch milliseconds are integral in the contract (`core-domain.md` §1.2 `AgentEvent.ts` epoch ms; §1.10
date rule), so `ts` is a `DateTime<Utc>` read with `serde_with::TimestampMilliSeconds`, and that adapter's
strict integer form cannot parse the float spelling. `Extra` keeps the event's unknown keys (including their
float spellings). Skipped by id.

## `term/http.json` — `post-clamp-low`

Request body sends `"cols":10.7,"rows":5.9`. The runtime coerced the floats (floor then clamp) into the PTY size;
clients send integers, so `cols`/`rows` are `u16` (`gateway-api.md` §6 terminal frames, `TermSize` is
integer-valued). Skipped by id.

## `term/http.json` — `post-clamp-nonnumber`

Request body sends `"cols":"132","rows":1e400` (string and out-of-range float). The runtime coerced both to the
80x24 default; `u16` cannot accept these spellings. Skipped by id.

## `term/ws.json` — `claim-a-sized`

Client frame sends `"cols":81.9`. Same coercion as `post-clamp-low`; the `claim` frame's `cols`/`rows` are `u16`.
Skipped by id.

## `term/ws.json` — `garbage-ignored`

The step's client frames send `{"type":"resize","cols":"90","rows":30}` among deliberately malformed frames; a
string `cols` is not a frame a client sends. Skipped by id.

## `gateway/users-tenancy.json` — persona replaced

Copy of `golden/gateway/fixtures/users/tenancy.json` with the denylisted persona name replaced by
the neutral `user-b` (user id and its `devices` entry consistently; the record carries no `pgUrl` and no
`persona`). `alice` and `owner` are unchanged and match the recorded gateway runs (`http-plain.json`,
`http-tls.json`, `ws.json`, `term/*.json`), which pass the privacy scan unchanged.

## Null-valued object members — general comparison rule

Recorded servers and clients sometimes spell an absent optional field as an explicit `null`
(`"logprobs": null`, `"parent": null`, `"exit_code": null`), and the wire types model all of these as
`Option<T>`, because clients treat absent and `null` alike. The roundtrip comparison therefore removes
null-valued object members from both sides before comparing (`strip_nulls` in `roundtrip.rs`, one
recursive helper); array elements keep their positions. This is a comparison rule only — the
re-serialized bytes are unaffected.
