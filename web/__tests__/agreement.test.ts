import { describe, expect, it } from 'vitest';
import {
  hasAgreementContent,
  renderAgreement,
  sanitizeAgreementHtml,
} from '../agreement';

describe('agreement rendering', () => {
  it('reports whether there is content', () => {
    expect(hasAgreementContent(null)).toBe(false);
    expect(
      hasAgreementContent({ title: 'x', format: 'text', content: '  \n ' }),
    ).toBe(false);
    expect(
      hasAgreementContent({ title: 'x', format: 'text', content: 'hi' }),
    ).toBe(true);
  });

  it('escapes plain text and keeps it as one pre-wrapped block', () => {
    const html = renderAgreement({
      title: 't',
      format: 'text',
      content: '<b>a</b>\n2',
    });
    expect(html).toContain('class="agreement-plain"');
    expect(html).toContain('&lt;b&gt;a&lt;/b&gt;');
    expect(html).not.toContain('<b>');
  });

  it('renders the markdown subset', () => {
    const html = renderAgreement({
      title: 't',
      format: 'md',
      content: '# 标题\n\n- 一\n- 二\n\n**粗** [链接](https://example.com)',
    });
    expect(html).toContain('<h2>标题</h2>');
    expect(html).toContain('<li>一</li>');
    expect(html).toContain('<strong>粗</strong>');
    expect(html).toContain('<a href="https://example.com"');
  });

  it('strips scripts, event handlers, form controls and non-http links', () => {
    const html = sanitizeAgreementHtml(
      '<p onclick="x()">a</p><script>alert(1)</script>' +
        '<form><input value="x"></form>' +
        '<a href="javascript:alert(1)">b</a>' +
        '<a href="mailto:a@b.c">c</a><a href="#top">d</a>' +
        '<iframe src="https://evil"></iframe>' +
        '<img src="data:text/html,x" style="width:1px">' +
        '<img src="https://ok/i.png">',
    );
    expect(html).not.toContain('<script');
    expect(html).not.toContain('onclick');
    expect(html).not.toContain('<form');
    expect(html).not.toContain('<input');
    expect(html).not.toContain('javascript:');
    expect(html).not.toContain('<iframe');
    expect(html).not.toContain('data:');
    expect(html).not.toContain('style=');
    expect(html).toContain('href="mailto:a@b.c"');
    expect(html).toContain('href="#top"');
    expect(html).toContain('src="https://ok/i.png"');
  });

  it('removes comments', () => {
    expect(sanitizeAgreementHtml('a<!-- hidden -->b')).toBe('ab');
  });
});
