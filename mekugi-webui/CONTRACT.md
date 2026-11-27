# Web UI contract

Asset-host, connection, security, and browser-storage contract for
[Web UI requirements](SPEC.md#requirements). Transport/storage mechanics below are replaceable
across coordinated updates while preserving those requirements.

## Boundary contract

- **Assets-only host**: `mekugi web` serves only `GET`/`HEAD` page and embedded static assets
  (503 naming `just webui` when unbuilt). It never proxies machine APIs, holds
  credentials, or runs sessions. Index and manifest revalidate; hashed assets
  cache indefinitely.
- **CSP & untrusted content**: responses forbid framing, plugins, and base-URL changes, restrict
  scripts/styles to bundled assets (+ inline styles), and permit connections only to explicitly
  selected WS(S) connectors. Markdown raw HTML and unsafe schemes stay inert; remote Markdown
  images stay links rather than auto-loading. Bound user/tool images load via authenticated
  `/api/resources/*` HTTP requests as browser-owned blob URLs.
- **Direct authenticated connections**: each machine uses one JSON-RPC WebSocket control connection
  (first-message bearer auth), one lazy separately authenticated WS connection for
  attachment-bearing turn submissions, and concurrent bearer-authenticated HTTP requests for
  large content and images. Answers to server requests go back on the control connection,
  including images associated with individual question answers. Credentials never enter URLs,
  cookies, or UI-host requests.
- **Origin-scoped machine storage**: machine identities, labels, connector URLs, and tokens save in
  origin-scoped browser storage (scoped per machine record so concurrent tabs do not clobber
  unrelated machines). Removing a machine deletes its token and closes/cancels its connections;
  rotating a token retires active connections and cancels in-flight resource reads. Unreadable or
  failed storage shows a persistent warning while keeping current-page connections usable.
- **Delivery & navigation**: session state/snapshots always load fresh from the server (never
  restored from browser storage). Transport loss leaves unresolved mutations `unknown` with no
  automatic retry. Server-directed reattachment acquires the destination snapshot and settles
  originating-view effects before switching routes.
- **Composer presets**: Main reads and edits numbered slots through
  `thread/presets/list` and `thread/presets/write`. The server checks the
  previous slot value and active user configuration before saving. These
  methods grant no access to other configuration.
