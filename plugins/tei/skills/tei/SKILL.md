---
name: tei
description: TEI — Hugging Face Text Embeddings Inference server. Use when the user wants to embed text, rerank candidates, tokenize, or check the loaded model on their TEI instance. Talks directly to the TEI HTTP API.
---

# TEI

Hugging Face Text Embeddings Inference server — embed text, rerank candidates, tokenize. Talk to it directly over its HTTP API.

## How to call it

Read the base URL from `~/.lab/.env`, then curl the TEI API:

```bash
TEI_URL=$(grep -E '^TEI_URL=' ~/.lab/.env | cut -d= -f2-)
```

TEI runs unauthenticated by default. If your deployment is behind auth, add the appropriate header.

## Common operations

| Intent | Request |
|---|---|
| Health | `curl -sS "$TEI_URL/health" -w '\nHTTP %{http_code}\n'` |
| Loaded model / runtime info | `curl -sS "$TEI_URL/info"` |
| Embed text | `curl -sS -X POST "$TEI_URL/embed" -H 'Content-Type: application/json' -d '{"inputs":"hello world"}'` |
| Embed (batch) | `curl -sS -X POST "$TEI_URL/embed" -H 'Content-Type: application/json' -d '{"inputs":["a","b"]}'` |
| Sparse embeddings (SPLADE) | `curl -sS -X POST "$TEI_URL/embed_sparse" -H 'Content-Type: application/json' -d '{"inputs":"hello"}'` |
| Rerank against a query | `curl -sS -X POST "$TEI_URL/rerank" -H 'Content-Type: application/json' -d '{"query":"fruit","texts":["apple","car"]}'` |
| Tokenize | `curl -sS -X POST "$TEI_URL/tokenize" -H 'Content-Type: application/json' -d '{"inputs":"hello world"}'` |
| OpenAI-compatible embeddings | `curl -sS -X POST "$TEI_URL/v1/embeddings" -H 'Content-Type: application/json' -d '{"input":"hello","model":"tei"}'` |

`/embed` and `/rerank` depend on the loaded model: an **embedding** model serves `/embed` (and `/rerank` returns a `424 model is not a re-ranker` error), while a **reranker** model serves `/rerank`. Check `/info` to see which is loaded. `/rerank` accepts at most 100 texts per call — split larger batches across requests.

Full API reference: <https://huggingface.github.io/text-embeddings-inference/>

## Configuration

`TEI_URL` lives in `~/.lab/.env`. Verify connectivity:

```bash
curl -sS "$TEI_URL/health" -w '\nHTTP %{http_code}\n'
```

## When NOT to use this skill

- The user wants to *store or search* vectors — that's the `qdrant` skill.
- The phrase is a "teach/team" typo, or the Text Encoding Initiative XML standard — not this skill.
