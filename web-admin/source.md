# Web Admin Source Index

| Path | Kind | Responsibility | Boundary / Side effects |
| --- | --- | --- | --- |
| `api-client.js` | Interface | Provides the browser API command boundary, including deprecated manual lifecycle compatibility methods. | Sends authenticated HTTP requests; does not persist credentials. |
| `api.schema.json` | Interface | Lists the tested Web Admin API surface and deprecation metadata. | Handwritten client contract snapshot. |
| `app.js` | UI | Maintains local view state, Run/Runbooks route presentation, and persisted remediation lifecycle metadata with approval actions only. | Calls `api-client.js`; never writes lifecycle state during render. |
| `index.html` | UI | Defines the Penpot-aligned static admin shell, route navigation, and field-contained operational controls. | Loads browser-native modules and contains no credential storage. |
| `styles.css` | UI | Translates the Penpot operational shell into responsive, accessible navigation, cards, form-field grids, and lifecycle status presentation. | No runtime side effects. |
| `scripts/source.md` | Tooling | Indexes Web Admin validation and export scripts. | Separate tooling boundary. |
