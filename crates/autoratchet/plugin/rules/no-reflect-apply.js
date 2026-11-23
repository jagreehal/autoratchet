// Vendored from anti-slop@13909c9e07590f5a85d93c62712259fe14a883f7 (MIT). Regenerate with scripts/vendor-anti-slop.mjs.
const defineRule = (rule) => rule;

import { isGlobalReflectMethodCall } from '../shared/reflect-method.js';

/** Ban Reflect.apply, which bypasses ordinary typed function calls. */
export const noReflectApplyRule = defineRule({
  meta: {
    type: 'problem',
    docs: {
      description:
        'Disallow Reflect.apply; call typed functions directly or model dynamic dispatch behind an interface.',
    },
    messages: {
      reflectApply:
        'Replace `Reflect.apply` with a typed function call. Model dynamic dispatch behind a named interface.',
    },
  },
  createOnce(context) {
    return {
      CallExpression(node) {
        if (node.callee.type === 'Super' || node.callee.type === 'V8IntrinsicExpression') return;
        if (isGlobalReflectMethodCall(context.sourceCode, node.callee, 'apply')) {
          context.report({ node, messageId: 'reflectApply' });
        }
      },
    };
  },
});
