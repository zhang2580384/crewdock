import type { RouteLocationNormalizedLoaded } from 'vue-router';

export function consumeAuthToken(route: RouteLocationNormalizedLoaded) {
  const fragment = new URLSearchParams(window.location.hash.replace(/^#/, '')).get('token') || '';
  const query = typeof route.query.token === 'string' ? route.query.token : '';
  const token = fragment || query;
  if (!token) return '';

  const cleanUrl = new URL(window.location.href);
  cleanUrl.hash = '';
  cleanUrl.searchParams.delete('token');
  window.history.replaceState(window.history.state, '', `${cleanUrl.pathname}${cleanUrl.search}`);
  return token;
}
