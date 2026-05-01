import schemaJson from './generated/config.schema.json';
import Ajv, { type ValidateFunction } from 'ajv';
import addFormats from 'ajv-formats';

export const configSchema = schemaJson as Record<string, unknown>;

let cachedValidator: ValidateFunction | null = null;
let activeSchema: Record<string, unknown> = configSchema;

export function setSchema(schema: Record<string, unknown>): void {
  activeSchema = schema;
  cachedValidator = null;
}

export function getSchema(): Record<string, unknown> {
  return activeSchema;
}

export function getValidator(): ValidateFunction {
  if (cachedValidator) return cachedValidator;
  const ajv = new Ajv({ allErrors: true, strict: false, useDefaults: false });
  addFormats(ajv);
  cachedValidator = ajv.compile(activeSchema);
  return cachedValidator;
}
