package io.github.brownc98.loom

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

class WaitsTest {
    private fun wait(id: Long, kind: String, extra: String = "") = """{"id":$id,"kind":"$kind","text":"t","since":1,"answerable":true,"doc":null,"images":[]$extra}"""

    private fun node(id: Long, name: String, vararg waits: String) = """{"id":$id,"name":"$name","agent":null,"parent":null,"waits":[${waits.joinToString(",")}]}"""

    private fun read(vararg nodes: String) = Waits.read("[${nodes.joinToString(",")}]")!!

    private val all = Switches(ask = true, done = true)

    @Test
    fun kindsMapToTwoTones() {
        val asks = listOf("question", "approval", "trust", "auth", "input")
        asks.forEach { assertEquals(Tone.ASK, Wait(1, 1, "n", it, "").tone) }
        assertEquals(Tone.DONE, Wait(1, 1, "n", "done", "").tone)
    }

    @Test
    fun titleShowsTheQuestionIdOnlyForQuestions() {
        assertEquals("loom 총괄 · 질문 Q712", Wait(712, 1, "loom 총괄", "question", "").title("질문"))
        assertEquals("코디 화면정리 · 완료", Wait(9, 2, "코디 화면정리", "done", "").title("완료"))
        assertEquals("w · 승인 요청", Wait(9, 2, "w", "approval", "").title("승인 요청"))
    }

    @Test
    fun bodyIsThePromptSummaryElseThePlainTextOfParts() {
        val parts = ""","parts":[{"part":"text","text":"see "},{"part":"link","label":"plan","href":"https://x.test/p"},{"part":"doc","label":" M41"}]"""
        val prompt = ""","prompt":{"rows":[],"body":[],"summary":"rm -rf build","options":[]},"parts":[{"part":"text","text":"other"}]"""
        val waits = read(node(1, "a", wait(1, "done", parts), wait(2, "approval", prompt), wait(3, "question")))
        assertEquals(listOf("see plan M41", "rm -rf build", ""), waits.map { it.summary })
    }

    @Test
    fun aReadListsTheWaitsOfEachNode() {
        val waits = read(node(1, "a", wait(10, "question")), node(2, "b", wait(11, "done")))
        assertEquals(listOf(10L to "a", 11L to "b"), waits.map { it.id to it.name })
        assertEquals(listOf(1L, 2L), waits.map { it.node })
        assertEquals(null, Waits.read("not json"))
    }

    @Test
    fun onlyNodesAndBeatEventsMatter() {
        assertEquals(Heard.NODES, Waits.heard("""{"kind":"nodes"}"""))
        assertEquals(Heard.BEAT, Waits.heard("""{"kind":"beat","quietMs":75000}"""))
        assertEquals(Heard.OTHER, Waits.heard("""{"kind":"message","child":1,"toward":"parent"}"""))
        assertEquals(Heard.OTHER, Waits.heard("not json"))
    }

    @Test
    fun firstRunOnlyRecordsWhatItSaw() {
        val waits = read(node(1, "a", wait(10, "question")))
        val plan = Waits.plan(waits, seen = null, shown = emptySet(), switches = all, front = false)
        assertEquals(emptyList<Wait>(), plan.post)
        assertEquals(setOf(10L), plan.seen)
    }

    @Test
    fun newWaitsPostOnceAndGoneWaitsCancel() {
        val waits = read(node(1, "a", wait(10, "question"), wait(11, "done")))
        val plan = Waits.plan(waits, seen = setOf(10L, 9L), shown = setOf(9L, 10L), switches = all, front = false)
        assertEquals(listOf(11L), plan.post.map { it.id })
        assertEquals(setOf(9L), plan.cancel)
        assertEquals(setOf(10L, 11L), plan.seen)
        val again = Waits.plan(waits, seen = plan.seen, shown = setOf(10L, 11L), switches = all, front = false)
        assertEquals(emptyList<Wait>(), again.post)
        assertEquals(emptySet<Long>(), again.cancel)
    }

    @Test
    fun aTappedNotificationDoesNotComeBack() {
        val waits = read(node(1, "a", wait(10, "question")))
        val plan = Waits.plan(waits, seen = setOf(10L), shown = emptySet(), switches = all, front = false)
        assertEquals(emptyList<Wait>(), plan.post)
    }

    @Test
    fun switchesFilterAndCancelTheirTone() {
        val waits = read(node(1, "a", wait(10, "approval"), wait(11, "done")))
        val asksOnly = Waits.plan(waits, seen = emptySet(), shown = emptySet(), switches = Switches(ask = true, done = false), front = false)
        assertEquals(listOf(10L), asksOnly.post.map { it.id })
        val doneOff = Waits.plan(waits, seen = setOf(10L, 11L), shown = setOf(10L, 11L), switches = Switches(ask = true, done = false), front = false)
        assertEquals(setOf(11L), doneOff.cancel)
        val none = Waits.plan(waits, seen = emptySet(), shown = emptySet(), switches = Switches(ask = false, done = false), front = false)
        assertEquals(emptyList<Wait>(), none.post)
        assertFalse(Switches(ask = false, done = false).any)
    }

    @Test
    fun nothingPostsWhileTheAppIsInFront() {
        val waits = read(node(1, "a", wait(10, "question")))
        val plan = Waits.plan(waits, seen = emptySet(), shown = emptySet(), switches = all, front = true)
        assertEquals(emptyList<Wait>(), plan.post)
        assertEquals(setOf(10L), plan.seen)
    }

    @Test
    fun fingerprintMatchesTheWebViewPinEncoding() {
        assertEquals("ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0", Door.fingerprint("abc".toByteArray()))
    }
}
