# cleargate-storage-oss

SeaORM-based persistence layer for ClearGate. Provides durable storage for flow definitions, versions, tags, and run events.

## Overview

This crate implements the storage traits defined by the flow engine using SeaORM, supporting both SQLite and PostgreSQL backends.

## Exports

| Store | Description |
|-------|-------------|
| `SeaOrmFlowStore` | CRUD operations for flow definitions and versions |
| `SeaOrmRunStore` | Persist and query flow run events |
| `SeaOrmTagStore` | Manage named tags pointing to flow versions |

## Database Schema

| Table | Purpose |
|-------|---------|
| `flow_head` | Current metadata for each flow |
| `flow_version` | Immutable snapshots of flow definitions |
| `flow_tag` | Named references to specific versions (e.g., `latest`, `prod`) |
| `run_event` | Ordered event log for each flow run |

## Supported Backends

- **SQLite** -- Zero-config local development and single-node deployments
- **PostgreSQL** -- Production-grade multi-node deployments

## Usage

```rust
use cleargate_storage_oss::{SeaOrmFlowStore, SeaOrmRunStore};

let db = Database::connect("sqlite://cleargate.db").await?;
let flow_store = SeaOrmFlowStore::new(db.clone());
let run_store = SeaOrmRunStore::new(db);
```

Migrations are datestamped and applied automatically on startup.

## License

Apache-2.0
