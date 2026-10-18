import type { Site } from './types';

const COMMON_DOMAIN_LABELS = new Set(['www', 'com', 'cn', 'net', 'org', 'io', 'co', 'app', 'dev', 'local']);

export function siteIconText(title: string, url: string, override?: string | null) {
  const manual = override?.trim();
  if (manual) return Array.from(manual).slice(0, 12).join('');

  const name = title.trim();
  const nameCharacters = Array.from(name);
  if (/^[a-z0-9][a-z0-9._-]*$/i.test(name) && nameCharacters.length <= 8) return name;
  const visualLength = iconVisualLength(name);
  if (name && !/\s/.test(name) && visualLength <= 8) return name;

  const domainLabel = usefulDomainLabel(url);
  if (domainLabel) return domainLabel;

  const tokens = name.match(/[a-z0-9][a-z0-9._-]*/gi) || [];
  const token = tokens.find((value) => Array.from(value).length >= 2 && Array.from(value).length <= 12);
  if (token) return token;
  return nameCharacters.slice(0, 4).join('') || 'WEB';
}

export function siteIconTextClass(text: string) {
  const visualLength = iconVisualLength(text);
  if (visualLength <= 4) return 'site-wordmark-short';
  if (visualLength <= 8) return 'site-wordmark-medium';
  return 'site-wordmark-long';
}

export function siteIconSources(site: Pick<Site, 'icon_url' | 'auto_icon_url'>) {
  return [site.icon_url?.trim(), site.auto_icon_url?.trim()]
    .filter((value): value is string => Boolean(value))
    .filter((value, index, values) => values.indexOf(value) === index);
}

function usefulDomainLabel(url: string) {
  try {
    const hostname = new URL(url).hostname.toLowerCase();
    if (/^\d+(?:\.\d+){3}$/.test(hostname) || hostname.includes(':')) return '';
    const labels = hostname.split('.').filter((label) => label && !COMMON_DOMAIN_LABELS.has(label));
    const preferred = [...labels].reverse().find((label) => (
      /^[a-z0-9-]+$/i.test(label)
      && /[a-z]/i.test(label)
      && label.length <= 12
    ));
    return preferred || '';
  } catch {
    return '';
  }
}

function iconVisualLength(value: string) {
  return Array.from(value).reduce((total, character) => (
    total + (/^[\x00-\x7f]$/.test(character) ? 1 : 2)
  ), 0);
}
