// Shared types for the editor API client.

import type { AppConfig } from './generated/types';

export type ApiBase = string;

export interface ProfileMeta {
  name: string;
  active?: boolean;
  mtime?: string;
  hash?: string;
  size?: number;
  [k: string]: unknown;
}

export interface Snapshot {
  timestamp: string;
  size?: number;
  [k: string]: unknown;
}

export interface ValidationIssue {
  field_path?: string;
  path?: string;
  level?: 'error' | 'warning';
  message: string;
  [k: string]: unknown;
}

export type ValidateResult =
  | { ok: true; errors?: ValidationIssue[] }
  | { ok: false; errors: ValidationIssue[] };

export interface ObsScene {
  name: string;
  [k: string]: unknown;
}
export interface ObsSource {
  name: string;
  [k: string]: unknown;
}
export interface ObsInput {
  name: string;
  [k: string]: unknown;
}

export interface ActionParam {
  name: string;
  kind?: 'string' | 'number' | 'integer' | 'boolean';
  picker?: 'obs.scene' | 'obs.source' | 'obs.input' | string;
  required?: boolean;
  default?: unknown;
}

export interface ActionDescriptor {
  name: string;
  description?: string;
  params?: ActionParam[];
}

export interface DriverDescriptor {
  name: string;
  description?: string;
}

export interface MidiPorts {
  inputs: string[];
  outputs: string[];
}

export interface LiveEvent {
  // Backend tags the variant via `event`; legacy code may also send `kind`.
  event?: 'hw_event' | 'connection' | 'config_reloaded' | 'page_changed' | string;
  kind?: string; // for hw_event: 'fader' | 'press' | 'release' | 'rotate' | 'axis' | 'encoder'
  control_id?: string;
  value?: number;
  ts?: number;
  target?: string;
  status?: 'up' | 'down';
  detail?: string;
  // For page_changed:
  index?: number;
  name?: string;
  [k: string]: unknown;
}

export type LiveHandler = (ev: LiveEvent) => void;

export class ApiError extends Error {
  constructor(public status: number, message: string, public body?: unknown) {
    super(message);
  }
}

export class ConflictError extends ApiError {
  constructor(public currentBody: string, public currentHash: string | undefined, message: string) {
    super(409, message);
  }
}

export type { AppConfig };
