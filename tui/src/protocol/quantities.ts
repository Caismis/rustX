/** Exact formatting of native unsigned quantities; no binary64 conversion. */
export function unsignedQuantity(value: string | number): bigint {
  if (typeof value === 'number') {
    if (!Number.isSafeInteger(value) || value < 0) throw new Error('Invalid unsigned quantity.');
    return BigInt(value);
  }
  if (!/^(0|[1-9][0-9]*)$/.test(value)) throw new Error('Invalid unsigned quantity.');
  return BigInt(value);
}

export function compactQuantity(value: string | number): string {
  const quantity = unsignedQuantity(value);
  if (quantity < 10_000n) return quantity.toString();
  const tenths = (quantity + 50n) / 100n;
  const fraction = tenths % 10n;
  return `${tenths / 10n}${fraction === 0n ? '' : `.${fraction}`}k`;
}

export function groupedQuantity(value: string | number): string {
  return unsignedQuantity(value).toLocaleString('en-US');
}
