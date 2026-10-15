export type Appearance = 'system' | 'light' | 'dark';

export const THEME_KEY = 'ring.theme';

/** Runs before first paint (inlined in the root layout, and pinned by hash in the page's CSP). Keep it ES5. */
export const themeBootScript = `try{var t=localStorage.getItem('${THEME_KEY}');var d=t==='dark'||(t!=='light'&&matchMedia('(prefers-color-scheme: dark)').matches);document.documentElement.dataset.theme=d?'dark':'light'}catch(e){}`;

export function readAppearance(): Appearance {
  try {
    const value = localStorage.getItem(THEME_KEY);
    return value === 'light' || value === 'dark' ? value : 'system';
  } catch {
    return 'system';
  }
}

export function applyAppearance(appearance: Appearance) {
  const dark = appearance === 'dark' || (appearance === 'system' && matchMedia('(prefers-color-scheme: dark)').matches);
  document.documentElement.dataset.theme = dark ? 'dark' : 'light';
}

export function saveAppearance(appearance: Appearance) {
  try {
    if (appearance === 'system') localStorage.removeItem(THEME_KEY);
    else localStorage.setItem(THEME_KEY, appearance);
  } catch {
    // Private browsing can refuse storage; the choice still applies to this tab.
  }
  applyAppearance(appearance);
}
