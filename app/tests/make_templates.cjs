// Rebuilds app/ui/scan-templates.js by averaging each letter across the labelled gene rows.
const fs = require('fs');
const path = require('path');
const { fixtures, templatesFrom } = require('./helpers.cjs');

const templates = templatesFrom(fixtures());
const lines = Object.entries(templates).map(([letter, shape]) => `  ${letter}: [${[...shape].map(v => Math.round(v * 99)).join(',')}],`);
const text = `// Made by app/tests/make_templates.cjs from real game frames. Values are 0 to 99.\nObject.assign(GeneScan.TEMPLATES, {\n${lines.join('\n')}\n});\n`;
fs.writeFileSync(path.join(__dirname, '..', 'ui', 'scan-templates.js'), text);
console.log(Object.keys(templates).join(' '));
