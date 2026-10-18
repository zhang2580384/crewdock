import type { Site } from './types';

export function siteLaunchHref(site: Site) {
  if (site.resource_type === 'website') return site.url;
  const config = site.connection_config;
  const params = new URLSearchParams({
    host: config.host || '',
    port: String(config.port || (site.resource_type === 'ssh' ? 22 : 3389)),
    username: config.username || '',
  });
  if (site.resource_type === 'ssh') {
    params.set('auth', config.auth_method || 'password');
    if (config.auth_method === 'key' && config.identity_file) params.set('identity', config.identity_file);
  } else {
    params.set('clipboard', config.redirect_clipboard === false ? '0' : '1');
    params.set('drives', config.redirect_drives ? '1' : '0');
  }
  return `crewdock://${site.resource_type}?${params.toString()}`;
}
