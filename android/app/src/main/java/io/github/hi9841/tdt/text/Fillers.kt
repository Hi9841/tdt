package io.github.hi9841.tdt.text

/**
 * Removes hesitation sounds ("um", "uh", "erm", "hmm") from a transcript.
 *
 * A port of TDT desktop `desktop/src/stt/fillers.rs`; keep the two in step. Only sounds that never carry
 * meaning are removed. Words such as "like", "you know", "so", and "ah" can be real speech, so they stay.
 * The commas the recognizer puts around a filler go with it, and a sentence that started on a filler still
 * starts with a capital letter.
 */
object Fillers {
    fun remove(text: String): String {
        if (text.split(Regex("\\s+")).none { isFiller(core(it).second) }) return text

        val out = StringBuilder(text.length)
        // Opening quotes or brackets of a removed filler move to the next word.
        val carriedLead = StringBuilder()
        var capitalizeNext = false

        for ((gap, token) in tokens(text)) {
            val (lead, word, trail) = core(token)
            if (!isFiller(word)) {
                if (out.isNotEmpty()) out.append(gap)
                out.append(carriedLead)
                carriedLead.clear()
                if (capitalizeNext) {
                    appendCapitalized(out, token)
                    capitalizeNext = false
                } else {
                    out.append(token)
                }
                continue
            }

            val startsSentence = endsSentence(out.toString().trimEnd())
            if (startsSentence && word.firstOrNull()?.isUpperCase() == true) capitalizeNext = true
            carriedLead.append(lead)

            val mark = terminalMark(trail)
            if (mark != null) {
                // "That's it, um." keeps its full stop: "That's it."
                if (out.endsWith(",") || out.endsWith(";")) out.setLength(out.length - 1)
                if (out.isNotEmpty() && !endsSentence(out.toString())) out.append(mark)
            } else if (',' in trail && out.endsWith(",")) {
                // "We should, uh, go" was one phrase: "We should go".
                out.setLength(out.length - 1)
            }
        }
        // A quote opened on a final filler has no word to land on, so it is dropped rather than left unbalanced.
        return out.toString()
    }

    /** Whitespace before each token, then the token. */
    private fun tokens(text: String): List<Pair<String, String>> =
        Regex("(\\s*)(\\S+)").findAll(text).map { it.groupValues[1] to it.groupValues[2] }.toList()

    /** Splits a token into leading punctuation, the word, and trailing punctuation. */
    private fun core(token: String): Triple<String, String, String> {
        val isWord = { c: Char -> c.isLetterOrDigit() || c == '\'' || c == '-' }
        val start = token.indexOfFirst(isWord).let { if (it < 0) token.length else it }
        val end = token.indexOfLast(isWord).let { if (it < 0) start else it + 1 }
        return Triple(token.substring(0, start), token.substring(start, end), token.substring(end))
    }

    private fun isFiller(word: String): Boolean {
        if (word.isEmpty() || word.length > 12) return false
        val lower = word.lowercase()
        val collapsed = StringBuilder()
        for (c in lower) if (collapsed.lastOrNull() != c) collapsed.append(c)
        return when (collapsed.toString()) {
            // um, umm, uh, uhh, uhm, uhmm, hm, hmm, erm, errm
            "um", "uh", "uhm", "hm", "erm" -> true
            // "err" is also a verb; only the bare sound is a filler.
            "er" -> lower == "er"
            // "mm" and "mmm", but not the letter "m".
            "m" -> lower.length >= 2
            else -> false
        }
    }

    private fun terminalMark(trail: String): Char? {
        if (".." in trail || '…' in trail) return null
        return trail.lastOrNull { it == '.' || it == '?' || it == '!' }
    }

    private fun endsSentence(text: String): Boolean =
        text.isEmpty() ||
            text.trimEnd('"', '\'', ')', ']', '”', '’').let {
                it.endsWith('.') || it.endsWith('?') || it.endsWith('!') || it.endsWith('\n')
            }

    private fun appendCapitalized(out: StringBuilder, token: String) {
        val leadLength = token.indexOfFirst { it.isLetterOrDigit() }.let { if (it < 0) token.length else it }
        out.append(token, 0, leadLength)
        val rest = token.substring(leadLength)
        if (rest.isNotEmpty()) {
            out.append(rest.substring(0, 1).uppercase())
            out.append(rest, 1, rest.length)
        }
    }
}
