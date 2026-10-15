# Ring web app: notes for agents

Read `README.md` here and `../udd/api.md` (the wire contract) and `../udd/design.md` (the screens) first.

## Silicon UI (the design system)

Build every screen with `@teamofsilicons/silicon-ui`. Rules: https://github.com/teamofsilicons/silicon-ui/blob/main/docs/rules.md

- Before writing UI, read `node_modules/@teamofsilicons/silicon-ui/components.json` (every component, its import
  path and exports) and the component's types in `node_modules/@teamofsilicons/silicon-ui/components/<name>/`.
  Live previews: https://ui.teamofsilicons.com/<name>.
- Import components from `@teamofsilicons/silicon-ui/<name>`. Never write a raw <button>, <input>, <select>,
  <textarea> or <dialog>; never copy, patch or restyle a component; never import its .module.css.
- In CSS, spacing, radius, colour and font-size come from tokens: var(--space-*), var(--radius-*), var(--…colour…),
  var(--text-*). No raw px, rem, hex, rgb() or oklch() for those.
- The product's look is set only in the theme file (`app/tokens.css`). Do not change it to make a screen work.
- If a component, variant or behaviour is missing: STOP and say what is missing. Do not build a workaround. The one
  marked exception lives in `components/ring/exceptions/` and is listed in `README.md`.
- Before finishing: `npm run lint` (eslint and `silicon-ui-check`) and `npm run build` pass.

## Ring rules

- No telemetry, analytics or crash reporting. Run Next only through `scripts/next.mjs`.
- No dynamic routes (static export): use query strings such as `/call?id=…`.
- Every mutation sends credentials and `X-Ring-Client: web` (`lib/api.ts` does this).
- Don't edit `../assets` or `../udd`; `scripts/assets.mjs` copies what the app needs.
