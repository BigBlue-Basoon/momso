// Loaded before the screen: fragments never go into storage or network requests.
let fragment = '';
if (typeof window !== 'undefined') {
  fragment = window.location.hash;
  if (fragment) window.history.replaceState(null, '', window.location.pathname + window.location.search);
}

export function takeOAuthFragment() {
  const value = fragment;
  fragment = '';
  return value;
}

export function hasOAuthFragment() { return fragment !== ''; }
