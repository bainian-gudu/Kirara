import { t } from '../i18n';
import { invoke } from '../host';
import { renderAgreement } from '../agreement';
import type { AgreementConfig } from '../state';
import { Dialog } from '../ui/Dialog';

/**
 * 协议全文弹窗。正文经过白名单净化后注入；正文里的链接不在安装窗口里导航：
 * 外部链接交给系统浏览器打开，页内锚点照常，其余一律忽略。
 */
export function AgreementPanel({
  agreement,
  onClose,
  onAccept,
}: {
  agreement: AgreementConfig;
  onClose: () => void;
  onAccept: () => void;
}) {
  function onClick(event: MouseEvent) {
    const target = event.target as HTMLElement | null;
    const anchor = target?.closest?.('a');
    if (!anchor) return;
    const href = anchor.getAttribute('href') ?? '';
    if (href.startsWith('#')) return;
    event.preventDefault();
    if (/^(?:https?:|mailto:)/i.test(href)) {
      void invoke('launch', { path: href });
    }
  }

  return (
    <Dialog
      title={<div class="title">{agreement.title || t('ready.eula')}</div>}
      footer={
        <>
          <button class="btn btn-install btn-install-2rd neutral" onClick={onClose}>
            {t('dialog.agreement_close')}
          </button>
          <button class="btn btn-install" onClick={onAccept}>
            {t('dialog.agreement_accept')}
          </button>
        </>
      }
    >
      <div
        class="agreement-body"
        onClick={onClick}
        dangerouslySetInnerHTML={{ __html: renderAgreement(agreement) }}
      />
    </Dialog>
  );
}
