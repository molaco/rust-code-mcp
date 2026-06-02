# Graph-Truth Search and Embedding Proposal

## Goal

Make the graph the source of truth for code identity, and build embedding inputs from graph Items instead of parser-only chunks.

Exact graph tools such as imports, exports, usages, calls, signatures, and surface queries should stay structural. They do not need embeddings or a vector database.

Embeddings are for similarity workflows only:

- semantic code search
- `similar_to_item`
- `semantic_overlaps`
- codemap semantic reranking

## Core Contract

Introduce a graph-derived embedding input contract:

```text
GraphEmbeddingInput {
    unit_id,
    node_id,
    unit_kind,
    split_part,
    split_total,
    qualified_name,
    item_kind,
    file,
    byte_span,
    line_range,
    input_text,
    input_hash,
    truncated,
    input_policy_version,
}
```

`unit_kind` distinguishes inputs that should not be treated as equivalent:

- `body`: canonical callable body input
- `signature`: declaration/signature input
- `container_summary`: summary for containers such as impls/modules/traits
- `split_body`: part of an oversized callable body

`GraphEmbeddingInput` is transient builder output. The vector index must not store full source documents. It stores vectors plus graph metadata.

`input_text` exists only long enough to feed the embedder, and optionally a separate text/BM25 index if we explicitly choose one. Vector rows should keep `input_hash`, source location, graph identity, and small preview/source references, not full source bodies.

## Vector Row Contract

The vector index stores rows, not documents:

```text
GraphVectorRow {
    unit_id,
    node_id,
    unit_kind,
    split_part,
    split_total,
    qualified_name,
    item_kind,
    file,
    byte_span,
    line_range,
    input_hash,
    truncated,
    input_policy_version,
    graph_id,
    vector,
}
```

## Stable Identity

`unit_id` should be stable across content edits:

```text
unit_id = hash(node_id + unit_kind + split_part + input_policy_version)
```

`input_hash` should represent the actual generated embedding input and drive freshness/cache invalidation.

Do not include `input_hash` in `unit_id`; doing so makes updates create new row identities and cleanup harder.

## Freshness

Graph embedding inputs and vector rows are valid only for the graph snapshot/fingerprint they were generated from.

This can be enforced as an invariant or stored explicitly in metadata, for example:

```text
graph_id or source_fingerprint
```

Any vector/cache row from a different graph snapshot is stale.

## Input Policy

All embedding inputs must be produced by one graph Item -> `GraphEmbeddingInput` builder.

That builder owns all embedding-input policy:

- how functions and methods are rendered
- how types are rendered
- how containers are summarized
- how oversized bodies are split or truncated
- how token limits are enforced
- how `input_policy_version` is assigned

No consumer should hand raw source spans directly to the embedder.

## Item Rendering Policy

Functions, methods, and associated functions:

- primary similarity unit is the callable body
- include qualified name, signature, item kind, file/location, and body
- enforce token budget before embedding

Oversized callable bodies:

- prefer structural splitting by block/statement
- otherwise truncate with `truncated = true`
- split rows keep the same `node_id`
- split rows get `unit_kind = split_body` and `split_part`

Impl blocks:

- do not embed the full impl body
- represent as `container_summary`
- include impl type, trait if any, and method/assoc item names or signatures
- actual method bodies are embedded separately as method/function inputs

Structs, enums, unions, traits, type aliases:

- prefer declaration/signature style inputs
- avoid embedding huge expanded source bodies
- include child/member names when useful
- for traits, method signatures are usually more useful than full default bodies unless explicitly requested by policy

Modules:

- do not embed full module source
- use summaries only if module-level retrieval is needed
- include child item names and public surface shape

## Token Budget

The builder must enforce the active embedding profile's budget for every input.

Minimum policy:

- estimate or count tokens for `input_text`
- reject, split, or truncate before calling `embed_documents`
- record `truncated`
- include token-policy changes in `input_policy_version`

Batching many inputs into one embedder call is still required for throughput, but batching is separate from splitting. Splitting handles one oversized input; batching handles many valid inputs.

## Cache and Vector Metadata

Use graph unit identity for cache/vector rows:

```text
node_id + unit_id + split_part + input_hash + embedder_identity + input_policy_version + graph_id
```

Vector rows should store enough metadata to return graph-native results:

- `unit_id`
- `node_id`
- `unit_kind`
- `split_part`
- `split_total`
- `qualified_name`
- `item_kind`
- `file`
- `byte_span`
- `line_range`
- `input_hash`
- `truncated`
- `input_policy_version`
- `graph_id` or source fingerprint

## Semantic Overlaps

Route `semantic_overlaps` through `GraphEmbeddingInput`.

Use canonical item-level inputs first. Do not compare split body parts directly as top-level Items unless results are aggregated back to `node_id`.

Recommended behavior:

- compare `body`, `signature`, and `container_summary` inputs according to policy
- use `split_body` only for oversized callables
- aggregate split results to `node_id`
- report Item-level overlaps, not raw split-part overlaps

## Vector Search

Route vector indexing through graph embedding inputs where a graph snapshot exists.

Search results should return `unit_id` and `node_id` directly. This removes file/line guessing bridges from `similar_to_item` and codemap seed resolution.

Parser-only chunk indexing should become a fallback for workspaces without a graph snapshot.

## Replacing the Old Index

The old primary path is:

```text
parser symbols -> CodeChunk -> embed -> vector row
```

Replace it with:

```text
graph snapshot -> graph Items -> GraphEmbeddingInput -> embed -> GraphVectorRow
```

What stays:

- the graph snapshot / LMDB hypergraph
- the embedder backends
- the vector store as storage for vectors
- batching
- optional exact text/BM25 search, if still wanted

What goes away as the primary path:

- parser-only `CodeChunk` identity
- file/line guessing to recover graph Items
- raw source-span embedding in graph similarity
- vector rows without `node_id`

Concrete replacement:

1. Build or refresh the graph snapshot first.
2. Enumerate graph Items from the snapshot.
3. Convert each Item into one or more `GraphEmbeddingInput` values.
4. Batch `input_text` values into the embedder.
5. Store only `GraphVectorRow` metadata plus vectors in the vector store.
6. Delete stale vector rows by `graph_id`, `node_id`, `unit_id`, and `input_hash`.
7. Query the vector store and return `node_id` / `unit_id`.
8. Resolve full source and exact relationships from the graph, not from the vector row.

## Migration Sequence

1. Define `GraphEmbeddingInput` and `GraphVectorRow`.
2. Build graph Item -> `GraphEmbeddingInput`.
3. Enforce token budget inside the builder.
4. Add cache/vector metadata keyed by graph unit identity.
5. Route `semantic_overlaps` through `GraphEmbeddingInput`.
6. Update vector indexing to write `GraphVectorRow` rows.
7. Update `similar_to_item` to use `node_id` and `unit_id`.
8. Add better structural splitting for oversized callable bodies.
9. Demote parser chunk indexing to fallback.

## Non-Goals

- Do not use embeddings for exact imports/exports/usages/calls.
- Do not embed full impl bodies.
- Do not keep raw parser chunks as the main search identity once graph inputs/vector rows exist.
- Do not allow consumers to bypass the graph input builder and call the embedder with arbitrary raw source.
- Do not store full source documents in the vector index.
