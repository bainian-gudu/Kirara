import { useState } from 'preact/hooks';
import { t } from '../i18n';
import { intent, type UiState } from '../state';
import { invoke } from '../host';
import { hasAgreementContent } from '../agreement';
import { AgreementPanel } from '../panels/AgreementPanel';
import { Checkbox } from '../ui/Checkbox';
import { IconEdit, IconShield } from '../ui/icons';

export function Ready({
  ui,
  onOpenSource,
}: {
  ui: UiState;
  onOpenSource: () => void;
}) {
  const uninstall = ui.mode === 'uninstall';
  const update = ui.mode === 'update';
  // Renderer-local view state (not in UiState). 内联了协议正文时必须手动勾选；
  // 没有正文时保持上游的默认勾选。`null` 表示用户还没动过，勾选态跟着协议是否内联
  // 走 —— 协议晚于挂载到达时不会把默认值定死在挂载那一刻。
  const agreement = ui.project.agreement;
  const hasAgreement = hasAgreementContent(agreement);
  const [agreeChoice, setAgreeChoice] = useState<boolean | null>(null);
  const agree = agreeChoice ?? !hasAgreement;
  const [showAgreement, setShowAgreement] = useState(false);
  const source = ui.sources.find((s) => s.uri === ui.options.source_uri);
  const mirrorc = ui.options.source_uri.startsWith('mirrorc://');
  const markedKey = ui.options.mirrorc_cdk
    ? ui.options.mirrorc_cdk.slice(0, 4) + '****'
    : t('ready.no_cdk');
  const showSources = ui.sources.length > 1 && !uninstall && !ui.offline;

  async function pickPath() {
    const path = await invoke<string>('pick_path');
    if (path) {
      await intent({ kind: 'set_path', path });
    }
  }

  const verb = uninstall
    ? t('ready.uninstall')
    : update
      ? t('ready.update')
      : t('ready.install');
  const destLabel = uninstall
    ? t('ready.uninstall_from')
    : update
      ? t('ready.update_to')
      : t('ready.install_to');

  return (
    <div class="actions">
      {!update && !uninstall ? (
        <div class="lnk">
          <Checkbox
            checked={ui.options.create_lnk}
            onChange={(value) => void intent({ kind: 'set_create_lnk', value })}
          />
          {t('ready.create_lnk')}
        </div>
      ) : null}
      {!update && !uninstall ? (
        <div class="read">
          <Checkbox checked={agree} onChange={setAgreeChoice} />
          {t('ready.agree')}
          {hasAgreement ? (
            <a class="agreement-link" onClick={() => setShowAgreement(true)}>
              {agreement?.title || t('ready.eula')}
            </a>
          ) : (
            <span class="agreement-link-off">{t('ready.eula')}</span>
          )}
        </div>
      ) : null}
      {uninstall ? (
        <div class="read">
          <Checkbox
            checked={ui.options.delete_user_data}
            onChange={(value) => void intent({ kind: 'set_delete_user_data', value })}
          />
          {t('ready.delete_user_data')}
        </div>
      ) : null}
      <div class="more">
        <span>
          {showSources ? (
            <>
              <span>{t('ready.from')} </span>
              {/* 无论当前是否 Mirror酱 都打开源列表，否则选了 Mirror酱
                  之后没有入口切回其他源；CDK 编辑走源列表里的确认流程。 */}
              <a onClick={onOpenSource} title={t('ready.select_source')}>
                {source?.name ?? ui.options.source_uri}
                {mirrorc ? `(${markedKey})` : null}
                <IconEdit />
              </a>
            </>
          ) : null}
          <span> {destLabel} </span>
        </span>
        {uninstall ? (
          <a>{ui.options.install_path}</a>
        ) : (
          <a onClick={() => void pickPath()} title={t('ready.change_path')}>
            {ui.options.install_path}
            <IconEdit />
          </a>
        )}
      </div>
      <button
        class="btn btn-install"
        disabled={!uninstall && !update && !agree}
        onClick={() => void intent({ kind: 'start' })}
      >
        {ui.needs_elevate ? (
          <IconShield />
        ) : null}
        {verb}
      </button>
      {showAgreement && agreement && hasAgreement ? (
        <AgreementPanel
          agreement={agreement}
          onClose={() => setShowAgreement(false)}
          onAccept={() => {
            setAgreeChoice(true);
            setShowAgreement(false);
          }}
        />
      ) : null}
    </div>
  );
}
