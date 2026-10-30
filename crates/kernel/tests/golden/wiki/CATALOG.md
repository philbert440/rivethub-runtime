# Wiki page model goldens (vendored from the TS diff corpus)

Each `<fn>.jsonl` line is one recorded call of the TS function and its result; MANIFEST.json gives the generator settings.

## wiki (slice 8w): 23 fns, 12,900 cases

The TS modules are `packages/wiki-core/dist/{model,page,identity}.js` and `services/mcp-sidecar/dist/wiki-read-format.js`.
The Rust functions live in `crates/wiki/src/` under the same names.

Removed generator classes (re-recorded 2026-10-11):
- Random strings (`string()`, `EDGE_STRINGS`) and mutate soup over slugs, titles, link text and whole pages are gone.
- Lone surrogates and `ﬁ`-style compatibility characters are gone.
- `__proto__`, `constructor` and numeric frontmatter keys are gone.
- Wrong-typed frontmatter values a YAML file cannot plausibly hold are gone (`slug: 5`, `aliases: {a: 1}`, sources given as
  strings or numbers, `last_verified: 20261010`).
- YAML anchors, aliases and complex keys are gone.
- `slug_token_prefix` counts of -1 and 1.5 are gone.
- Bare `##` lines are gone.

Realistic error cases are kept:
- Human frontmatter edits: a duplicate key, bad indentation, an unclosed `[`, a tab, a list or scalar instead of a map,
  `title: yes` / `123` / empty, a scalar `aliases`, `last_verified: today`.
- A missing closing `---` or a `---` with a trailing space.
- CRLF line endings, a BOM, and a page truncated mid-write by a crash. Truncation can split an emoji, which is the only
  source of `str16`.
- Bad citation dates (`yesterday`, `2026-1-1`) and broken `[[` links.
- Slugs derived from real titles (case, accents, `/`, `.`, `_`, doubled dashes) and an unknown `section` argument to
  `format_wiki_read`.

| fn | TS module:export | Rust counterpart | cases | notes |
|---|---|---|---|---|
| normalize_slug | model.js:normalizeSlug | slug.rs:normalize_slug | 1200 (0) | Slugs, titles and lines. Plain JSON |
| extract_wiki_links | model.js:extractWikiLinks | slug.rs:extract_wiki_links | 800 (0) | Plain JSON |
| parse_wiki_page | page.js:parseWikiPage | page.rs:parse_wiki_page | 600 (247) | Throws WikiParseError (missing frontmatter, title or slug, frontmatter not a map) and YAMLParseError (text from the `yaml` package). Tags: str16, undefined |
| serialize_wiki_page | page.js:serializeWikiPage | page.rs:serialize_wiki_page | 300 (0) | Plain JSON |
| apply_patch | page.js:applyPatch | page.rs:apply_patch | 300 (0) | Spread `[page?, patch]`. Tags: undefined |
| merge_pages | page.js:mergePages | page.rs:merge_pages | 200 (0) | Spread. Plain JSON |
| merge_summary_text | page.js:mergeSummaryText | page.rs:merge_summary_text | 800 (0) | Spread. Tags: str16 |
| cap_summary | page.js:capSummary | page.rs:cap_summary | 400 (0) | Spread. Plain JSON |
| demote_h2_headings | page.js:demoteH2Headings | page.rs:demote_h2_headings | 800 (0) | Plain JSON |
| build_wiki_search_text | page.js:buildWikiSearchText | page.rs:build_wiki_search_text | 300 (0) | Plain JSON |
| split_article_sections | page.js:splitArticleSections | page.rs:split_article_sections | 600 (0) | Plain JSON |
| join_article_sections | page.js:joinArticleSections | page.rs:join_article_sections | 400 (0) | Spread. Plain JSON |
| apply_article_patches | page.js:applyArticlePatches | page.rs:apply_article_patches | 600 (0) | Spread. Plain JSON |
| merge_article_bodies | page.js:mergeArticleBodies | page.rs:merge_article_bodies | 600 (0) | Spread. Plain JSON |
| parse_see_also | page.js:parseSeeAlso | page.rs:parse_see_also | 600 (0) | Plain JSON |
| parse_citations | page.js:parseCitations | page.rs:parse_citations | 800 (0) | Plain JSON |
| is_slug_variant | identity.js:isSlugVariant | identity.rs:is_slug_variant | 600 (0) | Spread. Plain JSON |
| prefer_canonical_slug | identity.js:preferCanonicalSlug | identity.rs:prefer_canonical_slug | 600 (0) | Spread. Plain JSON |
| slug_token_prefix | identity.js:slugTokenPrefix | identity.rs:slug_token_prefix | 600 (0) | Spread `[slug, n?]`, with n from 0 to 5. Plain JSON |
| find_stem_match | identity.js:findStemMatch | identity.rs:find_stem_match | 600 (0) | Spread. Tags: undefined |
| cluster_slugs_by_stem | identity.js:clusterSlugsByStem | identity.rs:cluster_slugs_by_stem | 500 (0) | The output is a Map. Tags: map |
| entities_overlap | identity.js:entitiesOverlap | identity.rs:entities_overlap | 400 (0) | Spread. Includes NFC/NFD `é`. Tags: undefined |
| format_wiki_read | wiki-read-format.js:formatWikiRead | read.rs:format_wiki_read | 300 (0) | Spread `[markdown, {slug, section?}]`. Includes oversized pages that hit the caps. Plain JSON |
