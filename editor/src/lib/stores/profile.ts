import { writable, get, derived, type Readable } from 'svelte/store';
import yaml from 'js-yaml';
import { api, type ProfileMeta, type ValidationIssue } from '$lib/api';
import type { AppConfig } from '$lib/generated/types';
import { getValidator } from '$lib/schema';

export interface ProfileState {
  name: string | null;
  meta: ProfileMeta | null;
  body: string;
  savedBody: string;
  parsed: AppConfig | null;
  errors: ValidationIssue[];
  serverErrors: ValidationIssue[];
  loading: boolean;
  saving: boolean;
}

const initial: ProfileState = {
  name: null,
  meta: null,
  body: '',
  savedBody: '',
  parsed: null,
  errors: [],
  serverErrors: [],
  loading: false,
  saving: false
};

export const profile = writable<ProfileState>(initial);

export const isDirty: Readable<boolean> = derived(profile, ($p) => $p.body !== $p.savedBody);
export const totalErrors: Readable<number> = derived(profile, ($p) =>
  [...$p.errors, ...$p.serverErrors].filter((e) => (e.level ?? 'error') === 'error').length
);
export const totalWarnings: Readable<number> = derived(profile, ($p) =>
  [...$p.errors, ...$p.serverErrors].filter((e) => e.level === 'warning').length
);

let validateTimer: ReturnType<typeof setTimeout> | null = null;
let serverValidateTimer: ReturnType<typeof setTimeout> | null = null;

function clientValidate(body: string): { parsed: AppConfig | null; errors: ValidationIssue[] } {
  let parsed: unknown;
  try {
    parsed = yaml.load(body);
  } catch (e) {
    const m = e instanceof Error ? e.message : String(e);
    return { parsed: null, errors: [{ message: `YAML: ${m}`, level: 'error', field_path: '' }] };
  }
  const validator = getValidator();
  const ok = validator(parsed);
  if (ok) return { parsed: parsed as AppConfig, errors: [] };
  const errs = (validator.errors ?? []).map((e) => ({
    field_path: e.instancePath || e.schemaPath,
    level: 'error' as const,
    message: `${e.instancePath || '/'} ${e.message ?? 'invalid'}`
  }));
  return { parsed: parsed as AppConfig | null, errors: errs };
}

async function runServerValidate(body: string): Promise<void> {
  try {
    const res = await api.validate.check(body);
    profile.update((p) =>
      p.body === body ? { ...p, serverErrors: res.ok ? [] : res.errors ?? [] } : p
    );
  } catch {
    /* server validate is best-effort */
  }
}

export const profileActions = {
  async load(name: string): Promise<void> {
    profile.update((p) => ({ ...p, loading: true }));
    try {
      const { meta, body } = await api.profiles.get(name);
      const { parsed, errors } = clientValidate(body);
      profile.set({
        name,
        meta,
        body,
        savedBody: body,
        parsed,
        errors,
        serverErrors: [],
        loading: false,
        saving: false
      });
      if (serverValidateTimer) clearTimeout(serverValidateTimer);
      serverValidateTimer = setTimeout(() => runServerValidate(body), 250);
    } catch (e) {
      profile.update((p) => ({
        ...p,
        loading: false,
        errors: [{ message: `Load failed: ${e instanceof Error ? e.message : String(e)}`, level: 'error' }]
      }));
    }
  },

  setBody(body: string): void {
    profile.update((p) => ({ ...p, body }));
    if (validateTimer) clearTimeout(validateTimer);
    validateTimer = setTimeout(() => {
      const { parsed, errors } = clientValidate(body);
      profile.update((p) => (p.body === body ? { ...p, parsed, errors } : p));
    }, 150);
    if (serverValidateTimer) clearTimeout(serverValidateTimer);
    serverValidateTimer = setTimeout(() => runServerValidate(body), 500);
  },

  // Directly mutate parsed config and re-serialize.
  patchParsed(mutator: (cfg: AppConfig) => void): void {
    const cur = get(profile);
    if (!cur.parsed) return;
    const cloned = JSON.parse(JSON.stringify(cur.parsed)) as AppConfig;
    mutator(cloned);
    const newBody = yaml.dump(cloned, { noRefs: true, lineWidth: 120 });
    profileActions.setBody(newBody);
  },

  async save(): Promise<{ ok: boolean; error?: string }> {
    const cur = get(profile);
    if (!cur.name) return { ok: false, error: 'No profile loaded' };
    profile.update((p) => ({ ...p, saving: true }));
    try {
      const meta = await api.profiles.save(cur.name, cur.body, cur.meta?.hash);
      profile.update((p) => ({
        ...p,
        savedBody: cur.body,
        meta: { ...(p.meta ?? { name: cur.name! }), ...meta },
        saving: false
      }));
      return { ok: true };
    } catch (e) {
      profile.update((p) => ({ ...p, saving: false }));
      const msg = e instanceof Error ? e.message : String(e);
      return { ok: false, error: msg };
    }
  },

  reset(): void {
    profile.set(initial);
  }
};
