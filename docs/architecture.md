# Architecture Dependency Policy

Sponzey Fleet follows Layered Architecture and Clean Architecture.

## Layers

```text
Interface -> Application -> Domain
Infrastructure -> Application/Domain contracts
Domain -> no outer layer dependency
```

## Crate Rules

- `fleet-domain` is the innermost crate.
- `fleet-domain` must not depend on async runtimes, HTTP frameworks, database clients, filesystem adapters, or process runners.
- `fleet-application` may depend on `fleet-domain`, but must not import concrete infrastructure implementations.
- `fleet-store` implements repository contracts and may depend on `fleet-domain` and `fleet-application`.
- `fleet-controller` and `fleet-agent` are interface library crates used by the single product binary.
- `fleet-cli` owns the only shipped binary target, `sponzey`.
- Controller and agent roles are selected with `fleet controller ...` and `fleet agent ...`, not separate executables.

## Current Fitness Check

Until a dedicated script exists, verify the domain dependency boundary with:

```bash
cargo tree -p fleet-domain
```

The output must not include framework, DB, HTTP, or async runtime dependencies such as `tokio`, `sqlx`, `axum`, or `reqwest`.
