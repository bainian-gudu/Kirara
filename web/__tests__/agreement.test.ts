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

  it('removes comments at every depth', () => {
    expect(sanitizeAgreementHtml('<p>a<!-- in --></p><!-- out -->b')).toBe(
      '<p>a</p>b',
    );
  });

  it('keeps the text of unknown tags but drops dangerous ones with their content', () => {
    const html = sanitizeAgreementHtml(
      '<section><p>a</p></section>' +
        '<script>alert(1)</script>' +
        '<style>p{}</style>',
    );
    // 未知容器拆掉标签，正文留下
    expect(html).not.toContain('<section');
    expect(html).toContain('<p>a</p>');
    // 危险标签连内容一起消失，脚本正文不会变成可见文本
    expect(html).not.toContain('alert(1)');
    expect(html).not.toContain('p{}');
  });

  it('drops attributes outside the allowlist', () => {
    const html = sanitizeAgreementHtml(
      '<p formaction="https://evil" autofocus data-x="1" onmouseover="x()">a</p>' +
        '<img src="https://ok/i.png" srcset="https://ok/2.png 2x" width="8">',
    );
    expect(html).not.toContain('formaction');
    expect(html).not.toContain('autofocus');
    expect(html).not.toContain('data-x');
    expect(html).not.toContain('onmouseover');
    expect(html).not.toContain('srcset');
    expect(html).toContain('src="https://ok/i.png"');
    expect(html).toContain('width="8"');
  });

  it('restricts ids and forces links to open outside the window', () => {
    const html = sanitizeAgreementHtml(
      '<p id="sec-1">b</p><p id="1 bad">c</p>' +
        '<a href="https://example.com" target="_self">d</a>',
    );
    expect(html).toContain('id="sec-1"');
    expect(html).not.toContain('1 bad');
    expect(html).toContain('target="_blank"');
    expect(html).not.toContain('_self');
    expect(html).toContain('rel="noreferrer noopener"');
  });
});
