import { useEffect, useState } from 'preact/hooks';
import { invoke, listen } from './host';
import { ready as i18nReady, t } from './i18n';
import {
  errorDialogArgs,
  intent,
  isDone,
  isFailed,
  isRunning,
  state,
  type Coded,
  type UiState,
} from './state';
import leftArt from './left.webp';
import { Ready } from './screens/Ready';
import { Running } from './screens/Running';
import { Done } from './screens/Done';
import { Failed } from './screens/Failed';
import { SourcePanel } from './panels/SourcePanel';
import { CdkPanel } from './panels/CdkPanel';
import { AgreementPanel } from './panels/AgreementPanel';
import { registerPluginBridge } from './plugin-host';
import { Dialog } from './ui/Dialog';
import { IconClose, IconMinimize } from './ui/icons';
import { Spinner } from './ui/Spinner';

type Panel = 'source' | 'cdk' | null;

function PromptModal({ ui }: { ui: UiState }) {
  const prompt = ui.pending;
  if (!prompt) return null;
  const title = t(`prompt.${prompt.kind}.title`, prompt.params);
  const message = t(`prompt.${prompt.kind}.message`, {
    ...prompt.params,
    items: prompt.items.join('\n'),
  });
  return (
    <Dialog
      title={<div class="title">{title}</div>}
      desc={<div class="desc">{message}</div>}
      footer={
        <>
          <button
            class="btn btn-install btn-install-2rd neutral"
            onClick={() => void intent({ kind: 'answer', id: prompt.id, ok: false })}
          >
            {t('dialog.cancel')}
          </button>
          <button
            class="btn btn-install"
            onClick={() => void intent({ kind: 'answer', id: prompt.id, ok: true })}
          >
            {t('dialog.ok')}
          </button>
        </>
      }
    >
      {prompt.items.length ? (
        <ul class="prompt-items">
          {prompt.items.map((item) => (
            <li key={item}>{item}</li>
          ))}
        </ul>
      ) : null}
    </Dialog>
  );
}

function Screen({
  ui,
  onOpenSource,
  onOpenCdk,
  agreeChoice,
  onAgreeChange,
  onOpenAgreement,
}: {
  ui: UiState;
  onOpenSource: () => void;
  onOpenCdk: () => void;
  agreeChoice: boolean | null;
  onAgreeChange: (value: boolean) => void;
  onOpenAgreement: () => void;
}) {
  const { phase } = ui;
  if (phase.kind === 'ready') {
    return (
      <Ready
        ui={ui}
        onOpenSource={onOpenSource}
        agreeChoice={agreeChoice}
        onAgreeChange={onAgreeChange}
        onOpenAgreement={onOpenAgreement}
      />
    );
  }
  if (isRunning(phase)) {
    return <Running ui={ui} progress={phase} />;
  }
  if (isDone(phase)) {
    return <Done result={phase} />;
  }
  if (isFailed(phase)) {
    return <Failed coded={phase} onOpenCdk={onOpenCdk} />;
  }
  return null;
}

export function App() {
  const ui = state.value;
  const [panel, setPanel] = useState<Panel>(null);
  // 点 Mirror 卡片时记下候选 URI；确定成功才提交，取消只关面板。
  const [cdkUri, setCdkUri] = useState<string | null>(null);
  const [copyReady, setCopyReady] = useState(false);
  const [showAgreement, setShowAgreement] = useState(false);
  // Renderer-local view state (not in UiState). 内联了协议正文时必须手动勾选；
  // 没有正文时保持上游的默认勾选。`null` 表示用户还没动过，勾选态跟着协议是否内联
  // 走 —— 协议晚于挂载到达时不会把默认值定死在挂载那一刻。
  const [agreeChoice, setAgreeChoice] = useState<boolean | null>(null);
  const agreement = ui.project.agreement;
  // 弹窗与主界面同处一层：弹窗自身不铺底色（见 layout.css 的 .dialog），
  // 打开时主界面整块让位，否则会从弹窗下面透出来。
  const overlay = panel !== null || showAgreement || ui.pending !== null;

  useEffect(() => {
    void i18nReady().then(() => setCopyReady(true));
    void invoke('window_show');
    let unsub: (() => void) | undefined;
    // session-plugin 监听必须先于 frontend_ready 就位，否则自动启动的
    // 会话发出插件请求时无人应答，只能等 60s 超时。
    void registerPluginBridge()
      .then(() =>
        listen<Coded>('ui-notice', (coded) => {
          void invoke('error_dialog', errorDialogArgs(coded));
        }),
      )
      .then((fn) => {
        unsub = fn;
        // 监听已就绪，向 host 看门狗上报页面可用。
        void invoke('frontend_ready');
      });
    return () => {
      unsub?.();
    };
  }, []);

  if (!ui || !copyReady) {
    return (
      <div class="main">
        <div class="init-loading">
          <Spinner size={40} />
        </div>
      </div>
    );
  }

  return (
    <div class="main">
      {ui.theme === 'css' ? <link rel="stylesheet" href="/theme.css" /> : null}
      <div class={`content ${ui.project.borderless ? 'borderless' : ''}`} hidden={overlay}>
        {ui.project.borderless ? (
          <div class="controls">
            <button class="cont-minimize" onClick={() => void invoke('window_minimize')}>
              <IconMinimize />
            </button>
            <button class="cont-close" onClick={() => void invoke('window_close')}>
              <IconClose />
            </button>
          </div>
        ) : null}
        <div class="image">
          <img src={ui.theme === 'image' ? '/theme.webp' : leftArt} alt={ui.project.title} />
        </div>
        <div class="right">
          <div class="title">{ui.project.title}</div>
          <div class="desc">{ui.project.description}</div>
          <Screen
            ui={ui}
            onOpenSource={() => setPanel('source')}
            onOpenCdk={() => setPanel('cdk')}
            agreeChoice={agreeChoice}
            onAgreeChange={setAgreeChoice}
            onOpenAgreement={() => setShowAgreement(true)}
          />
        </div>
      </div>
      {panel === 'source' ? (
        <SourcePanel
          ui={ui}
          onClose={() => setPanel(null)}
          onMirrorc={(uri) => {
            setCdkUri(uri);
            setPanel('cdk');
          }}
        />
      ) : null}
      {panel === 'cdk' ? (
        <CdkPanel
          ui={ui}
          verifyUri={cdkUri}
          onCancel={() => {
            setCdkUri(null);
            setPanel(null);
            void intent({ kind: 'cancel_cdk' });
          }}
          onConfirmed={() => {
            setCdkUri(null);
            setPanel(null);
          }}
        />
      ) : null}
      {showAgreement && agreement ? (
        <AgreementPanel
          agreement={agreement}
          onClose={() => setShowAgreement(false)}
          onAccept={() => {
            setAgreeChoice(true);
            setShowAgreement(false);
          }}
        />
      ) : null}
      {ui.pending ? <PromptModal ui={ui} /> : null}
    </div>
  );
}
