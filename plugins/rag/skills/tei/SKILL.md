---
name: tei
description: Hugging Face Text Embeddings Inference (TEI) — embeddings/reranking server (embed text, rerank candidates, tokenize). Use when the user asks to embed/rerank/tokenize against the TEI service. Talks directly to the TEI HTTP API. NOT for: 'teach/team' typos, or the Text Encoding Initiative XML standard.
---

# Hugging Face Text Embeddings Inference

Embeddings/reranking server — embed text, rerank candidates, tokenize. Part of the `rag` plugin alongside the `qdrant` skill. Talk to it directly over its HTTP API.

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
| Served model / runtime metadata | `curl -sS "$TEI_URL/info"` |
| Embed text (one or many) | `curl -sS -X POST "$TEI_URL/embed" -H 'Content-Type: application/json' -d '{"inputs":["a","b"]}'` |
| Sparse (SPLADE-style) embeddings | `curl -sS -X POST "$TEI_URL/embed_sparse" -H 'Content-Type: application/json' -d '{"inputs":"hello"}'` |
| Rerank texts against a query | `curl -sS -X POST "$TEI_URL/rerank" -H 'Content-Type: application/json' -d '{"query":"fruit","texts":["apple","car"]}'` |
| Tokenize | `curl -sS -X POST "$TEI_URL/tokenize" -H 'Content-Type: application/json' -d '{"inputs":"hello world"}'` |
| OpenAI-compatible embeddings | `curl -sS -X POST "$TEI_URL/v1/embeddings" -H 'Content-Type: application/json' -d '{"input":"hello","model":"tei"}'` |

Notes:
- `/embed` vs `/rerank` depends on the loaded model. An **embedding** model serves `/embed`; calling `/rerank` against it returns `424 model is not a re-ranker`. Check `/info` to confirm what is loaded.
- `/rerank` accepts at most 100 texts per call — split larger batches across multiple requests.

Full API reference: <https://huggingface.github.io/text-embeddings-inference/>

## Configuration

`TEI_URL` lives in `~/.lab/.env`. Verify connectivity:

```bash
curl -sS "$TEI_URL/health" -w '\nHTTP %{http_code}\n'
```

All TEI operations are read-only / non-mutating — no destructive actions.

## When NOT to use this skill

- The user is asking about Qdrant — load the `qdrant-vector-search` or `qdrant-quality` skill in this same plugin.
- The user wants to store/search vectors rather than generate embeddings — that's Qdrant.
- The phrase is a "teach/team" typo, or the Text Encoding Initiative XML standard — not this skill.
