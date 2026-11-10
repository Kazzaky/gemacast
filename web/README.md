# Gemacast website

Vue 3, TypeScript, Vite, and Tailwind CSS website for product information and
release downloads. The mobile React application is in
[`gemacast-mobile/`](../gemacast-mobile/).

## Commands

Run from `web/` with Bun and Node.js `^22.18.0` or `>=24.12.0`, as specified in
[package.json](package.json).

| Command | Operation |
| --- | --- |
| `bun install --frozen-lockfile` | Install locked dependencies |
| `bun run dev` | Start the Vite development server |
| `bun run type-check` | Check Vue and TypeScript types |
| `bun run lint:check` | Run Oxlint and ESLint without changing files |
| `bun run lint` | Run Oxlint and ESLint with automatic fixes |
| `bun run format:check` | Check formatting under `src/` without changing files |
| `bun run format` | Format files under `src/` |
| `bun run test:unit --run` | Run Vitest once |
| `bun run build` | Type-check and build into `dist/` |
| `bun run preview` | Serve the production build locally |

Vitest is configured with jsdom; no website test files are currently present.
The repository [CI frontend job](../.github/workflows/ci.yml) checks the mobile
application, not this website.

## Source map

| Path | Content |
| --- | --- |
| `src/App.vue`, `src/views/LandingView.vue` | Page composition |
| `src/components/about/` | Introduction, features, demonstrations |
| `src/components/download/` | Platform downloads |
| `src/components/header/`, `src/components/footer/` | Navigation and FAQ |
| `src/components/shared/` | Shared components |
| `src/core/links.ts` | Repository, contact, and release asset URLs |
| `src/composables/` | Menu, accordion, and breakpoint logic |
| `src/assets/`, `public/` | Images, styles, and public metadata |

Project documentation: [README](../README.md),
[CONTRIBUTING](../CONTRIBUTING.md).
