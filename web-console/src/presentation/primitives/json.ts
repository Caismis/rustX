/** Keep original JSON number tokens when binary64 cannot represent their spelling. */
type RawNumber = { readonly rawJSON: string };
const json = JSON as typeof JSON & {
  rawJSON(source: string): RawNumber;
  isRawJSON(value: unknown): value is RawNumber;
  parse(text: string, reviver: (key: string, value: unknown, context: { source?: string }) => unknown): unknown;
};
export const isJsonNumber = (value: unknown): value is RawNumber => json.isRawJSON(value);
export function parseJson(text: string): unknown {
  return json.parse(text, (_key: string, value: unknown, context: { source?: string }) => typeof value === 'number' && context.source !== undefined && JSON.stringify(value) !== context.source
    ? json.rawJSON(context.source) : value);
}
