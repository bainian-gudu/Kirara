/**
 * 用户协议渲染：把打包期内联的正文（text / markdown / html）转成可安全注入的
 * HTML 片段。正文来自打包配置，但仍统一过一遍白名单净化：安装器窗口能触发提权
 * IPC，任何情况下都不让脚本、表单、内嵌外部内容进界面。
 */
import type { AgreementConfig } from './state';

/** 整体删除（含内容）的标签：可执行、可提交、可嵌入外部内容、非排版。 */
const FORBID_TAGS = new Set([
  'script',
  'style',
  'link',
  'meta',
  'base',
  'title',
  'template',
  'noscript',
  'form',
  'input',
  'button',
  'select',
  'textarea',
  'option',
  'optgroup',
  'iframe',
  'frame',
  'frameset',
  'object',
  'embed',
  'applet',
  'param',
  'svg',
  'math',
  'canvas',
  'audio',
  'video',
  'source',
  'track',
  'marquee',
]);

/** 一律剥掉的属性；`on*` 前缀的事件处理器另判。 */
const FORBID_ATTR = new Set([
  'style',
  'srcdoc',
  'formaction',
  'data',
  'background',
  'srcset',
  'ping',
  'action',
  'method',
  'http-equiv',
]);

/** 只允许 http(s) / mailto / 页内锚点，杜绝 `javascript:` / `data:` / `file:`。 */
const ALLOWED_URI = /^(?:https?:|mailto:|#)/i;

/** 取值受 `ALLOWED_URI` 约束的属性。 */
const URI_ATTRS = new Set([
  'href',
  'src',
  'cite',
  'poster',
  'longdesc',
  'usemap',
  'xlink:href',
]);

/** 白名单净化：排版标签保留，可执行 / 可提交 / 外链内容剥掉。 */
export function sanitizeAgreementHtml(html: string): string {
  const doc = new DOMParser().parseFromString(html, 'text/html');
  const body = doc.body;
  for (const el of Array.from(body.querySelectorAll('*'))) {
    const tag = el.tagName.toLowerCase();
    if (FORBID_TAGS.has(tag)) {
      el.remove();
      continue;
    }
    for (const attr of Array.from(el.attributes)) {
      const name = attr.name.toLowerCase();
      if (FORBID_ATTR.has(name) || name.startsWith('on')) {
        el.removeAttribute(attr.name);
        continue;
      }
      if (URI_ATTRS.has(name) && !ALLOWED_URI.test(attr.value.trim())) {
        el.removeAttribute(attr.name);
      }
    }
  }
  for (const node of Array.from(body.childNodes)) {
    if (node.nodeType === Node.COMMENT_NODE) {
      node.remove();
    }
  }
  return body.innerHTML;
}

/** HTML 转义 */
export function escapeHtml(input: string): string {
  return input
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

/** 行内标记：代码、粗体、斜体、删除线、链接 */
function renderInline(text: string): string {
  let out = escapeHtml(text);
  out = out.replace(/`([^`]+)`/g, '<code>$1</code>');
  out = out.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
  out = out.replace(/(^|[^*\w])\*([^*\n]+)\*/g, '$1<em>$2</em>');
  out = out.replace(/~~([^~]+)~~/g, '<del>$1</del>');
  // 链接只放行 http(s)；正则按转义后的文本匹配（引号已是 &quot;）。
  // 点击行为由 AgreementPanel 拦下交给系统浏览器，不在 WebView 里导航。
  out = out.replace(
    /\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g,
    '<a href="$2" target="_blank" rel="noreferrer noopener">$1</a>',
  );
  return out;
}

/**
 * 极简 Markdown 渲染（标题 / 段落 / 列表 / 引用 / 分隔线 / 代码块），
 * 够协议这类纯文档使用，不引入额外依赖。
 */
export function renderMarkdown(source: string): string {
  const lines = source.replace(/\r\n/g, '\n').split('\n');
  const out: string[] = [];
  let listType: 'ul' | 'ol' | null = null;
  let paragraph: string[] = [];
  let code: string[] | null = null;

  const flushParagraph = () => {
    if (!paragraph.length) return;
    out.push(`<p>${paragraph.map(renderInline).join('<br />')}</p>`);
    paragraph = [];
  };
  const flushList = () => {
    if (!listType) return;
    out.push(`</${listType}>`);
    listType = null;
  };

  for (const raw of lines) {
    const line = raw.replace(/\s+$/, '');

    if (code) {
      if (/^\s*```/.test(line)) {
        out.push(`<pre><code>${escapeHtml(code.join('\n'))}</code></pre>`);
        code = null;
      } else {
        code.push(raw);
      }
      continue;
    }
    if (/^\s*```/.test(line)) {
      flushParagraph();
      flushList();
      code = [];
      continue;
    }
    if (!line.trim()) {
      flushParagraph();
      flushList();
      continue;
    }

    const heading = /^(#{1,6})\s+(.*)$/.exec(line);
    if (heading) {
      flushParagraph();
      flushList();
      // `#` 映射到 h2，避免与弹窗自身的标题层级冲突
      const level = Math.min(heading[1].length + 1, 6);
      out.push(`<h${level}>${renderInline(heading[2])}</h${level}>`);
      continue;
    }
    if (/^\s*(?:-{3,}|\*{3,}|_{3,})\s*$/.test(line)) {
      flushParagraph();
      flushList();
      out.push('<hr />');
      continue;
    }

    const item = /^\s*(?:[-*+]|\d+[.)])\s+(.*)$/.exec(line);
    if (item) {
      flushParagraph();
      const want: 'ul' | 'ol' = /^\s*\d+[.)]\s+/.test(line) ? 'ol' : 'ul';
      if (listType !== want) {
        flushList();
        out.push(`<${want}>`);
        listType = want;
      }
      out.push(`<li>${renderInline(item[1])}</li>`);
      continue;
    }

    const quote = /^\s*>\s?(.*)$/.exec(line);
    if (quote) {
      flushParagraph();
      flushList();
      out.push(`<blockquote>${renderInline(quote[1])}</blockquote>`);
      continue;
    }

    flushList();
    paragraph.push(line.trim());
  }

  if (code) {
    out.push(`<pre><code>${escapeHtml(code.join('\n'))}</code></pre>`);
  }
  flushParagraph();
  flushList();
  return out.join('\n');
}

/** 协议是否有可展示的内容 */
export function hasAgreementContent(
  agreement?: AgreementConfig | null,
): boolean {
  return Boolean(agreement?.content?.trim());
}

/** 渲染协议为经过净化的 HTML 片段 */
export function renderAgreement(agreement?: AgreementConfig | null): string {
  if (!hasAgreementContent(agreement)) return '';
  const content = agreement!.content;
  const format = (agreement!.format ?? 'text').trim().toLowerCase();

  let html: string;
  if (format === 'html' || format === 'htm') {
    html = content;
  } else if (format === 'markdown' || format === 'md') {
    html = renderMarkdown(content);
  } else {
    // 纯文本：保留原始换行与缩进
    html = `<div class="agreement-plain">${escapeHtml(content)}</div>`;
  }
  return sanitizeAgreementHtml(html);
}
