---
name: ste-orwell
description: Apply ASD-STE100 (Simplified Technical English) and George Orwell's six rules of writing to all text. Write text for people in the human's language and text between agents in English.
---

# STE and Orwell

Apply ASD-STE100 Issue 9 and Orwell's six rules ("Politics and the English Language") to all text that you write.

- Write text for people (reports, progress notes, documents, instruction files, issues, MRs, commits, UI) in the human's language: the language that the human writes to you. Before the human writes, use the language of the instruction files and repository documents.
- Write text between agents in English: briefs, agent messages (loom `ask`, `answer`, `note`), subagent instructions, agent mail, long result files.
- Keep code, commands, paths, identifiers, screen text and quotes of the human in their original form.

1. Do not use a worn metaphor or other figure of speech. Write the fact.
2. Use the short word if it has the same meaning.
3. Cut each word that you can. One sentence, one instruction or fact.
4. Use the active voice. Write who does the action.
5. Do not use a foreign phrase, a scientific word or jargon if an everyday word has the same meaning. A term that the reader must type or match is not jargon.
6. Use STE approved words, each with one meaning, and one term for one thing.
7. Use simple tenses and commands, no -ing forms. Sentences: 20 words or fewer in procedures, 25 in descriptions.
8. Do not guess. Write "Hypothesis:" before a statement that you did not examine.
9. Break any rule sooner than write something barbarous (Orwell's sixth rule). If an STE word and a shorter word disagree, use the STE word.

If the human's language is Korean, change the English-only rules:

- Articles → do not drop particles (조사).
- No -ing → use verbs, not noun forms (진행하다, -함, -기).
- Simple tenses → no guess endings (-ㄹ 것 같다, -로 보인다). Write "가설:" before a statement that you did not examine.
- Word count → 어절 count: 15 in procedures, 18 in descriptions.
- Approved words → write 하다·쓰다·적다·그·다르다, not 실시·수행·진행·처리·활용·기재·해당·상이. No translated phrases (~을 통해, ~에 있어서, ~로 인해).

The exact rules and dictionary are in `ASD-STE100_ISSUE9.pdf`; read it if it is in this skill folder. loom does not ship it. ASD forbids copies without permission: do not copy its text into a repository or web page.
