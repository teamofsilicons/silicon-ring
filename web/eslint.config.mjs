import { defineConfig, globalIgnores } from 'eslint/config';
import nextVitals from 'eslint-config-next/core-web-vitals';
import nextTs from 'eslint-config-next/typescript';
import siliconUi from '@teamofsilicons/silicon-ui/eslint';

export default defineConfig([
  ...nextVitals,
  ...nextTs,
  globalIgnores(['.next/**', 'out/**', 'generated/**', 'public/**', 'next-env.d.ts']),
  // Local exceptions to Silicon UI are listed in README.md; each sits in its own file and says when it goes away.
  ...siliconUi({ ignores: ['components/ring/exceptions/**'] }),
]);
