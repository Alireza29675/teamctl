// Run with: node --test crates/teamctl/assets/web/app.test.cjs
// Exercise the trust boundary, rather than mirroring visual implementation.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { markdown, inlineMarkdown, safeLink, safeAssetPath, escapeHtml, timestamp } = require('./app.js');

test('raw message HTML remains inert, including attributes and closing tags', () => {
  const result = markdown('<img src=x onerror="alert(1)">\n</p><script>top.location="https://evil.test"</script>');
  assert(!result.includes('<img'));
  assert(!result.includes('<script'));
  assert(result.includes('&lt;img'));
  assert(result.includes('&quot;'));
});
test('Markdown links accept web and email destinations', () => {
  for (const target of ['https://example.test/page?a=1&b=2', 'http://localhost:3000', 'mailto:hello@example.test']) assert(safeLink(target));
  assert(inlineMarkdown('[read](https://example.test)').includes('rel="noopener noreferrer"'));
});
test('Markdown links reject active schemes, relative navigation, and obfuscation', () => {
  for (const target of ['javascript:alert(1)', 'JaVaScRiPt:alert(1)', 'data:text/html,<script>', 'file:///etc/passwd', 'blob:https://example.test/x', '//evil.test', '/api/state', 'java\nscript:alert(1)']) {
    assert.equal(safeLink(target), null, target);
  }
  assert(!markdown('[danger](javascript:alert)').includes('href='));
});
test('link labels and target quotes cannot escape their attributes', () => {
  const result = markdown('[<img src=x>](https://example.test/"onclick="x)');
  assert(!result.includes('<img'));
  assert(!result.includes('"onclick='));
});
test('code spans and blocks do not interpret embedded markup', () => {
  assert.equal(inlineMarkdown('`<script>alert(1)</script>`'), '<code>&lt;script&gt;alert(1)&lt;/script&gt;</code>');
  const result = markdown('```html\n<img src=x onerror="alert(1)">\n```');
  assert(result.includes('<pre>'));
  assert(result.includes('code-label'));
  assert(!result.includes('<img'));
});
test('unterminated code fences preserve content safely', () => {
  const result = markdown('```\n<script>');
  assert.equal(result, '<pre><code>&lt;script&gt;</code></pre>');
});
test('rich content supports headings, emphasis, quotes, lists, and tables', () => {
  const result = markdown('# Hello\n\n**bold** and *italic*\n\n> quoted\n\n- first\n- second\n\n| A | B |\n| --- | --- |\n| 1 | 2 |');
  for (const tag of ['<h1>', '<strong>', '<em>', '<blockquote>', '<ul>', '<li>', '<table>', '<th>', '<td>']) assert(result.includes(tag), tag);
});
test('table cells escape raw HTML', () => {
  assert(!markdown('| Heading |\n| --- |\n| <img src=x> |').includes('<img'));
});
test('attachment fetch paths remain inside the authenticated assets API', () => {
  assert.equal(safeAssetPath({ id: '123-abcd' }), '/api/assets/123-abcd');
  assert.equal(safeAssetPath({ url: '/api/assets/123-abcd' }), '/api/assets/123-abcd');
  for (const url of ['https://evil.test/api/assets/a', '//evil.test/api/assets/a', '/api/assets/../state', '/api/assets/%2e%2e', '/api/assets/%2fetc', '/api/assets/a?token=secret', '/api/assets/a#fragment', '/api/assets/..', '/api/assets/.']) assert.equal(safeAssetPath({ url }), null, url);
});
test('HTML escaping covers all attribute delimiters', () => {
  assert.equal(escapeHtml('"\'&<>'), '&quot;&#39;&amp;&lt;&gt;');
});
test('timestamps handle database seconds and ISO strings', () => {
  assert.equal(timestamp(1700000000).valueOf(), 1700000000000);
  assert.equal(timestamp('2026-10-18T12:00:00Z').toISOString(), '2026-10-18T12:00:00.000Z');
  assert.equal(timestamp('not-a-date'), null);
});
