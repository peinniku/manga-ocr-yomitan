// Same cases as the desktop's glossary tests (crates/app/src/glossary.rs).
import assert from 'node:assert/strict';
import { test } from 'node:test';

import { glossaryLines, lineKind } from '../src/gloss-lines.js';

test('flattens structured content like the desktop app', () => {
  const g = [
    {
      type: 'structured-content',
      content: [
        { tag: 'span', content: '見出し' },
        { tag: 'div', content: ['一つ目'] },
        { tag: 'ul', content: [{ tag: 'li', content: 'a' }, { tag: 'li', content: 'b' }] },
        { tag: 'ruby', content: ['漢', { tag: 'rt', content: 'かん' }] },
      ],
    },
    'plain',
  ];
  assert.deepEqual(glossaryLines(g), ['見出し', '一つ目', '・a', '・b', '漢（かん）', 'plain']);
});

test('classifies lines like the desktop app', () => {
  assert.equal(lineKind('＊もよお・す③⓪(モヨホス)【催す】', 0), 'headword');
  assert.equal(lineKind('㊀計画して、会などを開く。', 3), 'sense');
  assert.equal(lineKind('「涙を―」', 4), 'example');
  assert.equal(lineKind('（他五）〈なにヲ―〉', 1), 'note');
  assert.equal(lineKind('特定の問題について、集まった人が意見を述べあって議論をすること。', 2), 'text');
});
