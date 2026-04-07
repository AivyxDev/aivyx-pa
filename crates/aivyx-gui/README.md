# aivyx-gui

**Aivyx Personal Assistant — Web GUI**

SvelteKit-based web interface for the Aivyx PA. Connects to the PA HTTP API server
(`aivyx serve`, port 3100) and mirrors the TUI feature-for-feature in a browser UI.

---

## Stack

| Layer | Technology |
|---|---|
| Framework | SvelteKit 2 + Svelte 5 |
| Styling | TailwindCSS v4 (Stitch design system) |
| API | Fetch + EventSource (SSE) → `aivyx-pa` HTTP API |
| Fonts | Space Grotesk · Inter · JetBrains Mono |

## Design System

Uses the same **Stitch** palette as the TUI (`theme.rs`):
- **Primary** `#FFB77D` — amber/gold (actions, active items)
- **Secondary** `#CCC1E6` — lavender (secondary info, tags)
- **Sage** `#4D8B6A` — success, heartbeat, approved
- **Accent Glow** `#FFC864` — warnings, pending approvals

## Running

```bash
# 1. Start the PA backend (from aivyx-pa root)
cargo run -- serve

# 2. Start the GUI (from this directory)
npm install
npm run dev
# → http://localhost:5173
```

## Views

| Route | TUI Equivalent | API Endpoint |
|---|---|---|
| `/` | Home | `GET /api/dashboard` + `GET /api/notifications/history` |
| `/chat` | Chat | `POST /api/chat` (SSE) · `GET /api/sessions` |
| `/goals` | Goals | `GET /api/goals` |
| `/missions` | Missions | `GET /api/dashboard` |
| `/approvals` | Approvals | `GET /api/approvals` · `POST /approve\|deny` |
| `/activity` | Activity | `GET /api/notifications/history` + SSE |
| `/audit` | Audit | `GET /api/audit` · `GET /api/metrics` |
| `/memory` | Memory | `GET /api/memories` · `DELETE /api/memories/:id` |
| `/settings` | Settings | `GET /api/settings` · `PUT /api/settings/toggle` |

## Architecture

```
src/
├── lib/
│   ├── shared/
│   │   ├── api/
│   │   │   ├── client.ts     # Typed fetch + SSE helpers
│   │   │   └── types.ts      # TypeScript interfaces matching api.rs structs
│   │   ├── components/       # Shared UI primitives (StatCard, StatusBadge…)
│   │   └── stores/
│   │       ├── dashboard.ts  # Polls /api/dashboard every 30s
│   │       ├── notifications.ts  # SSE stream → live ring buffer
│   │       └── theme.ts      # Dark/light toggle
│   └── pa/
│       ├── components/
│       │   └── layout/       # Sidebar, TopBar, StatusBar
│       └── stores/           # Per-view stores
└── routes/
    ├── +layout.svelte         # Root: starts stores
    ├── (app)/                 # Authenticated shell (Sidebar + TopBar)
    │   ├── +layout.svelte
    │   ├── +page.svelte       # Dashboard
    │   ├── chat/
    │   ├── goals/
    │   ├── missions/
    │   ├── approvals/
    │   ├── activity/
    │   ├── audit/
    │   ├── memory/
    │   └── settings/
    ├── genesis/               # First-run wizard
    └── unlock/                # Passphrase unlock screen
```
