// REST + WS clients for the xtouch-gw editor backend (same-origin).

import {
  ApiError,
  ConflictError,
  type ProfileMeta,
  type Snapshot,
  type ValidateResult,
  type ObsScene,
  type ObsSource,
  type ObsInput,
  type DriverDescriptor,
  type ActionDescriptor,
  type MidiPorts,
  type LiveEvent,
  type LiveHandler
} from './api-types';

export * from './api-types';

const API_BASE: string = (import.meta.env.VITE_API_BASE as string | undefined) ?? '';

async function apiFetch(path: string, init?: RequestInit): Promise<Response> {
  return fetch(`${API_BASE}${path}`, {
    credentials: 'omit',
    ...init,
    headers: {
      Accept: 'application/json',
      ...(init?.body ? { 'Content-Type': 'application/json' } : {}),
      ...(init?.headers ?? {})
    }
  });
}

async function asJson<T>(res: Response): Promise<T> {
  if (res.status === 204) return undefined as unknown as T;
  const text = await res.text();
  if (!text) return undefined as unknown as T;
  try {
    return JSON.parse(text) as T;
  } catch {
    return text as unknown as T;
  }
}

async function callJson<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await apiFetch(path, init);
  if (!res.ok) {
    let body: unknown;
    try {
      body = await res.clone().json();
    } catch {
      try {
        body = await res.text();
      } catch {
        /* ignore */
      }
    }
    throw new ApiError(res.status, `${init?.method ?? 'GET'} ${path}: ${res.status}`, body);
  }
  return asJson<T>(res);
}

const u = (s: string) => encodeURIComponent(s);

async function getProfileFlex(name: string): Promise<{ meta: ProfileMeta; body: string }> {
  const res = await apiFetch(`/api/profiles/${u(name)}`);
  if (!res.ok) throw new ApiError(res.status, `GET profile failed: ${res.status}`);
  const j = (await res.json()) as { meta: ProfileMeta; body: string };
  return { meta: j.meta, body: j.body };
}

async function saveProfile(name: string, body: string, expectedHash?: string): Promise<ProfileMeta> {
  const res = await apiFetch(`/api/profiles/${u(name)}`, {
    method: 'PUT',
    headers: expectedHash ? { 'If-Match': expectedHash } : {},
    body: JSON.stringify({ body, expected_hash: expectedHash })
  });
  if (res.status === 409) {
    let p: { body?: string; hash?: string; current_body?: string; current_hash?: string } = {};
    try {
      p = await res.json();
    } catch {
      /* ignore */
    }
    throw new ConflictError(p.current_body ?? p.body ?? '', p.current_hash ?? p.hash, 'Profile changed externally');
  }
  if (!res.ok) throw new ApiError(res.status, `Save failed: ${res.status}`);
  return asJson<ProfileMeta>(res);
}

async function readHistory(name: string, ts: string): Promise<{ timestamp: string; body: string }> {
  const res = await apiFetch(`/api/profiles/${u(name)}/history/${u(ts)}`);
  if (!res.ok) throw new ApiError(res.status, `GET history failed: ${res.status}`);
  const ct = res.headers.get('content-type') ?? '';
  if (ct.includes('application/json')) {
    const j = (await res.json()) as { timestamp?: string; body?: string; yaml?: string };
    return { timestamp: j.timestamp ?? ts, body: j.body ?? j.yaml ?? '' };
  }
  return { timestamp: ts, body: await res.text() };
}

async function obsList<K extends string, T>(
  path: string,
  key: K
): Promise<{ connected: boolean } & { [P in K]: T[] }> {
  const empty = { connected: false, [key]: [] } as { connected: boolean } & { [P in K]: T[] };
  const res = await apiFetch(path);
  if (res.status === 503) return empty;
  if (!res.ok) throw new ApiError(res.status, `${path}: ${res.status}`);
  const data = (await res.json()) as T[] | Record<K, T[]>;
  const list = Array.isArray(data) ? data : (data as Record<K, T[]>)[key] ?? [];
  return { connected: true, [key]: list } as { connected: boolean } & { [P in K]: T[] };
}

export const api = {
  profiles: {
    list: () => callJson<ProfileMeta[]>('/api/profiles'),
    get: getProfileFlex,
    save: saveProfile,
    create: (name: string, body?: string, source?: string) =>
      callJson<ProfileMeta>('/api/profiles', {
        method: 'POST',
        body: JSON.stringify({ name, body, source })
      }),
    duplicate: (name: string, newName: string) =>
      callJson<ProfileMeta>(`/api/profiles/${u(name)}/duplicate`, {
        method: 'POST',
        body: JSON.stringify({ new_name: newName })
      }),
    rename: (name: string, newName: string) =>
      callJson<void>(`/api/profiles/${u(name)}/rename`, {
        method: 'POST',
        body: JSON.stringify({ new_name: newName })
      }),
    delete: (name: string) => callJson<void>(`/api/profiles/${u(name)}`, { method: 'DELETE' }),
    activate: (name: string) => callJson<void>(`/api/profiles/${u(name)}/activate`, { method: 'POST' }),
    active: () => callJson<{ name: string }>('/api/profiles/active'),
    history: (name: string) => callJson<Snapshot[]>(`/api/profiles/${u(name)}/history`),
    historyRead: readHistory,
    historyRestore: (name: string, ts: string) =>
      callJson<ProfileMeta>(`/api/profiles/${u(name)}/history/${u(ts)}/restore`, { method: 'POST' })
  },
  schema: {
    get: () => callJson<Record<string, unknown>>('/api/schema')
  },
  validate: {
    check: (body: string) =>
      callJson<ValidateResult>('/api/validate', { method: 'POST', body: JSON.stringify({ body }) })
  },
  midi: {
    ports: () => callJson<MidiPorts>('/api/midi/ports')
  },
  obs: {
    scenes: () => obsList<'scenes', ObsScene>('/api/obs/scenes', 'scenes'),
    sources: (scene: string) =>
      obsList<'sources', ObsSource>(`/api/obs/scenes/${u(scene)}/sources`, 'sources'),
    inputs: () => obsList<'inputs', ObsInput>('/api/obs/inputs', 'inputs')
  },
  page: {
    get: () => callJson<{ index: number; name: string }>('/api/page'),
    setActive: (index: number) =>
      callJson<void>('/api/page', {
        method: 'POST',
        body: JSON.stringify({ index })
      })
  },
  drivers: {
    async list(): Promise<DriverDescriptor[]> {
      try {
        const res = await apiFetch('/api/drivers');
        if (!res.ok) return [];
        const j = (await res.json()) as DriverDescriptor[] | { drivers: DriverDescriptor[] };
        return Array.isArray(j) ? j : j.drivers ?? [];
      } catch {
        return [];
      }
    },
    async actions(name: string): Promise<ActionDescriptor[]> {
      try {
        const res = await apiFetch(`/api/drivers/${u(name)}/actions`);
        if (!res.ok) return [];
        const j = (await res.json()) as ActionDescriptor[] | { actions: ActionDescriptor[] };
        return Array.isArray(j) ? j : j.actions ?? [];
      } catch {
        return [];
      }
    }
  }
};

export class LiveSocket {
  private ws: WebSocket | null = null;
  private listeners = new Set<LiveHandler>();
  private connHandlers = new Set<(c: boolean) => void>();
  private reconnectMs = 1500;
  private closed = false;
  private _connected = false;
  private url: string;

  constructor(url?: string) {
    if (url) this.url = url;
    else {
      const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
      this.url = `${proto}//${location.host}/api/live`;
    }
  }

  get connected(): boolean {
    return this._connected;
  }

  open(): void {
    if (this.closed || this.ws) return;
    try {
      const ws = new WebSocket(this.url);
      this.ws = ws;
      ws.onopen = () => this.setConnected(true);
      ws.onmessage = (msg) => {
        try {
          const ev = JSON.parse(typeof msg.data === 'string' ? msg.data : '') as LiveEvent;
          this.listeners.forEach((l) => l(ev));
        } catch {
          /* ignore */
        }
      };
      ws.onerror = () => {
        /* close handler reconnects */
      };
      ws.onclose = () => {
        this.ws = null;
        this.setConnected(false);
        if (!this.closed) setTimeout(() => this.open(), this.reconnectMs);
      };
    } catch {
      this.setConnected(false);
      if (!this.closed) setTimeout(() => this.open(), this.reconnectMs);
    }
  }

  subscribe(h: LiveHandler): () => void {
    this.listeners.add(h);
    return () => this.listeners.delete(h);
  }

  onConnected(h: (c: boolean) => void): () => void {
    this.connHandlers.add(h);
    h(this._connected);
    return () => this.connHandlers.delete(h);
  }

  emit(ev: LiveEvent): void {
    this.listeners.forEach((l) => l(ev));
  }

  close(): void {
    this.closed = true;
    this.ws?.close();
    this.ws = null;
  }

  private setConnected(c: boolean): void {
    this._connected = c;
    this.connHandlers.forEach((l) => l(c));
  }
}

export const liveSocket = new LiveSocket();
