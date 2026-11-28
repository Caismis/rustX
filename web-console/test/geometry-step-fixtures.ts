/** Construct independent STEP product roots from the analytic cube fixture. */
export function stepRootCases(cube: string) {
  const [header, data] = cube.split(/\r?\nDATA;\r?\n/);
  const [entities] = data.split(/\r?\nENDSEC;/);
  const second = entities.replace(/#(\d+)/g, (_, index: string) => `#${Number(index) + 1000}`);
  const model = (next: string) => `${header}\nDATA;\n${entities}\n${next}\nENDSEC;\nEND-ISO-10303-21;\n`;
  const declaration = /^#1003 = SHAPE_DEFINITION_REPRESENTATION[^\r\n]+/m;
  const representation = /^#1010 = ADVANCED_BREP_SHAPE_REPRESENTATION[^\r\n]+/m;
  const wireframe = (items: string) => second.replace(representation,
    `#1010 = GEOMETRICALLY_BOUNDED_WIREFRAME_SHAPE_REPRESENTATION('',(${items}),#1345);`);
  return {
    complete: model(second),
    metadata: model(second.replace(declaration, '')),
    partial: model(second.replace(declaration, '#1003 = SHAPE_DEFINITION_REPRESENTATION(#1004,$);')),
    wire: model(`${wireframe('#1011,#2001')}\n#2001 = GEOMETRIC_CURVE_SET('',(#2002));\n#2002 = POLYLINE('',(#1012,#1025));`),
    emptyWire: model(wireframe('#1011')),
  };
}
