import assert from 'node:assert/strict';
import test from 'node:test';
import { siteCategories } from '../src/site-category.ts';

test('category choices are unique, trimmed, and restricted to the form visibility', () => {
  const sites = [
    { visibility: 'personal', category: ' SEO ' },
    { visibility: 'personal', category: 'SEO' },
    { visibility: 'personal', category: '常用' },
    { visibility: 'personal', category: '  ' },
    { visibility: 'public', category: '团队工具' },
  ];
  assert.deepEqual(new Set(siteCategories(sites, 'personal')), new Set(['SEO', '常用']));
  assert.deepEqual(siteCategories(sites, 'public'), ['团队工具']);
  assert.deepEqual(siteCategories([], 'personal'), []);
});

test('category ordering stays stable when site order changes', () => {
  const sites = ['公司网络', 'SEO', '常用', '服务器提供商'].map((category) => ({
    visibility: 'personal', category,
  }));
  assert.deepEqual(siteCategories(sites, 'personal'), siteCategories([...sites].reverse(), 'personal'));
});
