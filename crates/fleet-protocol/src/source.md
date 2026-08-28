# Fleet Protocol Source Index

| Path | Kind | Responsibility | Boundary / Side effects |
| --- | --- | --- | --- |
| `lib.rs` | Protocol library | Defines versioned Controller-Agent wire messages, task payloads, and JSON encoding/decoding. | Serializes and parses wire data; owns no network transport or persistence. |
