// 영어 문구. 키 구조의 원천이다. ko.ts는 이 타입을 받으므로 키가 다르면 `tsc`가 실패한다.
const en = {
  languageName: { en: 'English', ko: '한국어' },
  settings: {
    language: 'Language',
    languageSystem: 'System',
    languageHint: '“System” follows the browser language. Anything other than Korean shows English.',
    rescan: 'Rescan',
    versionWarning: 'app-server {{installed}} · pinned {{pinned}} · asks approval for every loom command',
  },
  terminal: {
    gone: 'The daemon restarted, so this terminal no longer exists.',
  },
  problem: {
    refused: {
      emptyName: 'Enter a name.',
      nameTaken: '{{value}}: a parent or sibling already uses this name.',
      tooDeep: 'Nesting is too deep. Do not create more children.',
      briefTooLong: 'The message is {{value}} bytes. Shorten it and give a file path for longer material.',
      summaryTooLong: 'The summary is {{value}} bytes. Shorten it or use `loom done --file <path>`.',
      mailboxFull: 'The mailbox of {{value}} is full. Send again after the receiver reads it with `loom wait`.',
      notRelated: '{{value}}: not my parent or child. Messages go only between parent and child.',
      ended: '{{value}}: the node process has ended.',
      alreadyDone: 'This node is already done.',
      heldFull: 'Too many messages are held while this node has no parent. Send again after it gets a new parent.',
      cycle: 'A node cannot take itself or its descendant as parent.',
      notFound: '{{value}}: no node has this name.',
      ambiguous: '{{value}}: more than one node has this name. Change the parent on the screen.',
      launch: 'Could not start the node: {{value}}',
      db: 'Database error: {{value}}',
    },
    launch: {
      unknownAgent: '{{value}}: not in the agent catalog.',
      notInstalled: '{{agent}}: `{{command}}` is not on the daemon PATH. Install it, then click [Rescan] in Settings.',
      briefTooLong: 'The first prompt is {{len}} bytes. Keep it to {{limit}} bytes or less and give a file path for longer material.',
      server: 'Could not start `{{program}}`: {{cause}}',
      noServer: '{{value}}: could not start the server the node attaches to.',
      noBrief: '{{value}} does not take a first prompt. Leave it empty, create the node, then type in its terminal.',
    },
    noLogin: 'This login window no longer exists.',
    internal: 'Internal error: {{value}}',
  },
  usageProblem: {
    cliFailed: 'Could not run `{{command}}`: {{detail}}',
    cliFormat: '`{{value}}` returned an unknown format.',
    noReply: '`{{value}}` did not reply.',
    loginNotSupported: 'This login method (such as an API key) cannot show usage.',
    responseFormat: 'The usage response has an unknown format.',
    tokenExpired: 'The token expired. Run Claude once with this account to refresh it.',
    unavailable: 'Usage is not available now: {{value}}',
  },
};

export default en;
