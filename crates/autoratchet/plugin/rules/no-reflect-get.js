// Vendored from anti-slop@13909c9e07590f5a85d93c62712259fe14a883f7 (MIT). Regenerate with scripts/vendor-anti-slop.mjs.
const defineRule = (rule) => rule;

import { isGlobalReflectMethodCall } from '../shared/reflect-method.js';

/** Ban Reflect.get, which bypasses ordinary property access and useful type evidence. */
export const noReflectGetRule = defineRule({
  meta: {
    type: 'problem',
    docs: {
      description:
        'Disallow Reflect.get; use typed property access or parse dynamic input into a domain type.',
    },
    messages: {
      reflectGet:
        'Replace `Reflect.get` with typed property access. Parse dynamic input into a named domain type before reading it.',
    },
  },
  createOnce(context) {
    return {
      CallExpression(node) {
        if (node.callee.type === 'Super' || node.callee.type === 'V8IntrinsicExpression') return;
        if (isGlobalReflectMethodCall(context.sourceCode, node.callee, 'get')) {
          context.report({ node, messageId: 'reflectGet' });
        }
      },
    };
  },
});
