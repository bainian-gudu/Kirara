export type UnlistenFn = () => void;

export interface Event<T> {
  event: string;
  id: number;
  payload: T;
}

type InvokeMessage = {
  id: number;
  kind: 'invoke';
  cmd: string;
  args: unknown;
};

type ReplyMessage = {
  id: number;
  kind: 'reply';
  ok: boolean;
  data?: unknown;
  error?: unknown;
};

type EventMessage = {
  kind: 'event';
  event: string;
  payload: unknown;
};

type HostMessage = ReplyMessage | EventMessage;

type Pending = {
  resolve: (value: unknown) => void;
  reject: (reason?: unknown) => void;
};

type WebViewBridge = {
  postMessage: (message: unknown) => void;
  addEventListener: (
    type: string,
    handler: (event: { data: HostMessage | string }) => void,
  ) => void;
};

const pending = new Map<number, Pending>();
const listeners = new Map<string, Set<(payload: unknown) => void>>();
let nextId = 1;
let listening = false;

function webview(): WebViewBridge {
  const bridge = (
    window as unknown as { chrome?: { webview?: WebViewBridge } }
  ).chrome?.webview;
  if (!bridge) {
    throw new Error('WebView2 host bridge is not available');
  }
  return bridge;
}

function parseMessage(data: HostMessage | string): HostMessage | null {
  let message: unknown = data;
  if (typeof message === 'string') {
    try {
      message = JSON.parse(message);
    } catch {
      return null;
    }
  }
  if (!message || typeof message !== 'object') {
    return null;
  }
  return message as HostMessage;
}

function dispatchMessage(message: HostMessage) {
  if (message.kind === 'reply') {
    const waiter = pending.get(message.id);
    if (!waiter) {
      return;
    }
    pending.delete(message.id);
    if (message.ok) {
      waiter.resolve(message.data);
    } else {
      const error = message.error;
      if (
        error &&
        typeof error === 'object' &&
        'message' in error &&
        typeof error.message === 'string'
      ) {
        waiter.reject(new Error(error.message));
      } else {
        waiter.reject(error ?? new Error('host invoke failed'));
      }
    }
    return;
  }

  const callbacks = listeners.get(message.event);
  if (!callbacks) {
    return;
  }
  const event: Event<unknown> = {
    event: message.event,
    id: 0,
    payload: message.payload,
  };
  for (const callback of callbacks) {
    callback(event);
  }
}

function ensureListening() {
  if (listening) {
    return;
  }
  listening = true;
  webview().addEventListener('message', (event) => {
    const message = parseMessage(event.data);
    if (message) {
      dispatchMessage(message);
    }
  });
}

export async function invoke<T = unknown>(
  cmd: string,
  args?: unknown,
): Promise<T> {
  ensureListening();
  const id = nextId++;
  return new Promise<T>((resolve, reject) => {
    pending.set(id, {
      resolve: (value) => resolve(value as T),
      reject,
    });
    const message: InvokeMessage = {
      id,
      kind: 'invoke',
      cmd,
      args: args ?? {},
    };
    webview().postMessage(message);
  });
}

export async function listen<T>(
  event: string,
  handler: (event: Event<T>) => void,
): Promise<UnlistenFn> {
  ensureListening();
  let callbacks = listeners.get(event);
  if (!callbacks) {
    callbacks = new Set();
    listeners.set(event, callbacks);
  }
  callbacks.add(handler as (payload: unknown) => void);
  return () => {
    callbacks?.delete(handler as (payload: unknown) => void);
  };
}

export const sep = '\\';

export function getCurrentWindow() {
  return {
    close: () => invoke<void>('window_close'),
    show: () => invoke<void>('window_show'),
    minimize: () => invoke<void>('window_minimize'),
    setTitle: (title: string) =>
      invoke<void>('window_set_title', { title }),
    setDecorations: (decorations: boolean) =>
      invoke<void>('window_set_decorations', { decorations }),
  };
}
