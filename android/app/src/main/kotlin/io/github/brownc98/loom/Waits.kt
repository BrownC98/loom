package io.github.brownc98.loom

import org.json.JSONArray
import org.json.JSONObject

internal enum class Tone { ASK, DONE }

internal data class Wait(val id: Long, val node: Long, val name: String, val kind: String, val summary: String) {
    val tone get() = if (kind == DONE_KIND) Tone.DONE else Tone.ASK

    fun title(label: String) = if (kind == QUESTION_KIND) "$name · $label Q$id" else "$name · $label"

    private companion object {
        const val DONE_KIND = "done"
        const val QUESTION_KIND = "question"
    }
}

internal data class Switches(val ask: Boolean, val done: Boolean) {
    val any get() = ask || done

    fun allows(tone: Tone) = when (tone) {
        Tone.ASK -> ask
        Tone.DONE -> done
    }
}

internal enum class Heard { NODES, BEAT, OTHER }

internal data class Plan(val post: List<Wait>, val cancel: Set<Long>, val seen: Set<Long>)

internal object Waits {
    fun heard(event: String): Heard = when (runCatching { JSONObject(event).optString("kind") }.getOrNull()) {
        "nodes" -> Heard.NODES
        "beat" -> Heard.BEAT
        else -> Heard.OTHER
    }

    fun read(nodes: String): List<Wait>? {
        val list = runCatching { JSONArray(nodes) }.getOrNull() ?: return null
        return (0 until list.length()).mapNotNull { list.optJSONObject(it) }.flatMap { node ->
            val waits = node.optJSONArray("waits") ?: JSONArray()
            (0 until waits.length()).mapNotNull { waits.optJSONObject(it) }.map { wait(node.optLong("id"), node.optString("name"), it) }
        }
    }

    fun plan(waits: List<Wait>, seen: Set<Long>?, shown: Set<Long>, switches: Switches, front: Boolean): Plan {
        val wanted = waits.filter { switches.allows(it.tone) }
        val post = if (seen == null || front) emptyList() else wanted.filter { it.id !in seen && it.id !in shown }
        return Plan(post, shown - wanted.map { it.id }.toSet(), waits.map { it.id }.toSet())
    }

    private fun wait(node: Long, name: String, json: JSONObject) =
        Wait(json.optLong("id"), node, name, json.optString("kind"), summary(json))

    private fun summary(json: JSONObject): String {
        json.optJSONObject("prompt")?.let { return it.optString("summary") }
        val parts = json.optJSONArray("parts") ?: return ""
        return (0 until parts.length()).mapNotNull { parts.optJSONObject(it) }.joinToString("") { part ->
            if (part.optString("part") == "text") part.optString("text") else part.optString("label")
        }
    }
}
