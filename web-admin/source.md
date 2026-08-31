# Web Admin Source Index

| Path | Kind | Responsibility | Boundary / Side effects |
| --- | --- | --- | --- |
| `api-client.js` | Interface | Provides the browser API command boundary, including bounded catalog source/revision/document read calls and deprecated manual lifecycle compatibility methods. | Sends authenticated HTTP requests; does not persist credentials. |
| `api.schema.json` | Interface | Lists the tested Web Admin API surface and deprecation metadata. | Handwritten client contract snapshot. |
| `app.js` | UI | Maintains local route state and renders catalog source/revision/document metadata with explicit register, sync, and activate commands. | Calls `api-client.js`; never infers durable sync/activation state or persists credentials. |
| `index.html` | UI | Defines the Penpot-aligned static admin shell, route navigation, catalog explorer, and field-contained operational controls. | Loads browser-native modules and contains no credential storage. |
| `styles.css` | UI | Translates the Penpot operational shell into responsive, accessible navigation, catalog exploration, cards, form-field grids, and lifecycle status presentation. | No runtime side effects. |
| `scripts/source.md` | Tooling | Indexes Web Admin validation and export scripts. | Separate tooling boundary. |
