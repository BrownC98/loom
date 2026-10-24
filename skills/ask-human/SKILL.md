---
name: ask-human
description: The standard for how an agent asks the human. Use it for each question, choice, approval or permission. It tells you whom to ask and in which form. loom gives it to every node.
---

# Ask the human

The loom inbox shows the human only questions in this form.

- A node without a parent (the human started it) asks only with `loom ask "<question>"`. The question goes to the human's inbox.
- A node with a parent does not ask the human. Its `loom ask` goes to its parent, in English.
- Write one line: `<question> (A: <option> / B: <option>, Recommended: A)`. loom refuses options in other forms.
- Write the whole question in the human's language, also "Recommended".
- Rule change: give each option a "now → after" example.
- Give a yes/no question the tail too: `(A: Yes / B: No, Recommended: A)`. Without it, the inbox shows no buttons.
- Write a document for the human (mockup, report) with user-doc. Check it with `show.sh` and attach it with `--doc <file>`. The human cannot open a path in text. Do not open the human's browser.
  - A markdown link to that file on a word becomes the document link: `loom ask "Approve the [R9 mockup](<file>)?" --doc <file>`. Without a link, loom adds one at the end.
- In question and done text, put URLs and absolute paths only in markdown links.
- After `loom ask`, end your turn. The question gets a number (`Q12`). The human answers in the inbox or your terminal.
- Ask each question of a set with its own `loom ask`. Earlier questions stay open.
- An inbox answer comes as a `loom message` from human: "Answer to loom ask question Q12 (…): A". The question is closed. Do not call `loom answered`. Do what the answer tells.
- You decide if terminal text from the human answers an open question. It can start with the number: "Q12: A". While one is open, loom tells you in that turn how to close it.
- Do not ask only in chat. The human sees only inbox questions.
- The Claude question tool (`AskUserQuestion`) also puts the question in the inbox.
