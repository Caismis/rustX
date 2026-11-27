/** Construct independent STEP product roots from the analytic cube fixture. */
export function stepRootCases(cube: string) {
  const [header, data] = cube.split(/\r?\nDATA;\r?\n/);
  const [entities] = data.split(/\r?\nENDSEC;/);
  const second = entities.replace(/#(\d+)/g, (_, index: string) => `#${Number(index) + 1000}`);
  const model = (next: string) => `${header}\nDATA;\n${entities}\n${next}\nENDSEC;\nEND-ISO-10303-21;\n`;
  const declaration = /^#1003 = SHAPE_DEFINITION_REPRESENTATION[^\r\n]+/m;
  return {
    complete: model(second),
    metadata: model(second.replace(declaration, '')),
    partial: model(second.replace(declaration, '#1003 = SHAPE_DEFINITION_REPRESENTATION(#1004,$);')),
  };
}
