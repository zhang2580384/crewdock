import type { Site, SiteVisibility } from './types';

export function siteCategories(sites: Pick<Site, 'category' | 'visibility'>[], visibility: SiteVisibility): string[] {
  return [...new Set(sites
    .filter((site) => site.visibility === visibility)
    .map((site) => site.category.trim())
    .filter(Boolean))]
    .sort((left, right) => left.localeCompare(right, 'zh-CN'));
}
