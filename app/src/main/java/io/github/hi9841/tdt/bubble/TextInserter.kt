package io.github.hi9841.tdt.bubble

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Bundle
import java.text.Normalizer
import android.util.Log
import android.view.accessibility.AccessibilityNodeInfo

/** How the text reached the user. */
enum class InsertResult { SET_TEXT, PASTED, COPIED }

/** What a text field holds before an insert: nothing but its hint, the user's text, or TDT cannot tell. */
enum class Field { EMPTY, TEXT, UNSURE }

/**
 * Puts dictated text at the cursor of another app's text field.
 *
 * Order: set-text spliced at the selection (leaves the clipboard alone), then paste (works in many web views
 * where set-text fails), then copy to the clipboard so the user can paste by hand. When TDT cannot tell the
 * field's text from its hint ([Field.UNSURE]), paste goes first: the app then puts the words at its own cursor and
 * drops its own hint, and no text the user typed can be lost.
 */
class TextInserter(private val context: Context) {

    fun insert(node: AccessibilityNodeInfo?, text: String): InsertResult {
        val result = if (node != null && node.refresh() && node.isEditable && !node.isPassword) {
            val field = field(node)
            Log.i(TAG, "field before insert: ${describe(node)} read as $field")
            when {
                field == Field.UNSURE && paste(node, text) -> InsertResult.PASTED
                setText(node, text, if (field == Field.EMPTY) "" else node.text?.toString().orEmpty()) -> InsertResult.SET_TEXT
                field != Field.UNSURE && paste(node, text) -> InsertResult.PASTED
                else -> null
            }
        } else {
            null
        } ?: InsertResult.COPIED.also { copy(text) }
        // Field facts only, never its text: enough to see why an app's field takes text badly.
        if (node != null) {
            Log.i(
                TAG,
                "insert $result into ${node.packageName}/${node.className}: textLength=${node.text?.length ?: 0} " +
                    "hintLength=${node.hintText?.length ?: -1} showingHint=${node.isShowingHintText} " +
                    "selection=${node.textSelectionStart}..${node.textSelectionEnd}",
            )
        }
        return result
    }

    /** [current]: the field's text without its hint. */
    private fun setText(node: AccessibilityNodeInfo, text: String, current: String): Boolean {
        val start = node.textSelectionStart.takeIf { it in 0..current.length } ?: current.length
        val end = node.textSelectionEnd.takeIf { it in start..current.length } ?: start
        val insert = spaced(current.substring(0, start), text, current.substring(end))
        val updated = current.substring(0, start) + insert + current.substring(end)

        val arguments = Bundle().apply {
            putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, updated)
        }
        if (!node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, arguments)) return false
        // Some fields report success but drop or rewrite the text. Trust the field, not the return value.
        if (!node.refresh() || node.text?.toString() != updated) return false

        val cursor = start + insert.length
        node.performAction(
            AccessibilityNodeInfo.ACTION_SET_SELECTION,
            Bundle().apply {
                putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, cursor)
                putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, cursor)
            },
        )
        return true
    }

    private fun paste(node: AccessibilityNodeInfo, text: String): Boolean {
        copy(text)
        return node.performAction(AccessibilityNodeInfo.ACTION_PASTE)
    }

    private fun copy(text: String) {
        val clipboard = context.getSystemService(ClipboardManager::class.java)
        clipboard.setPrimaryClip(ClipData.newPlainText("TDT", text))
    }

    companion object {
        private const val TAG = "TDT"

        /**
         * What the field holds. Many chat apps report their hint ("Message") as the text of an empty field.
         * WhatsApp does it without a hint and without the hint flag, so the hint cannot be compared; but Android's
         * text field has a cursor (a selection) and the cursor actions only while it holds text. So a field that
         * reports short text with neither shows only its hint.
         */
        fun field(node: AccessibilityNodeInfo): Field {
            val text = node.text?.toString().orEmpty()
            if (text.isEmpty() || node.isShowingHintText) return Field.EMPTY
            node.hintText?.toString()?.let { hint ->
                // Compared after [normalized]: the reported hint can differ from the text in ways the eye cannot see.
                if (normalized(text) == normalized(hint)) return Field.EMPTY
            }
            if (node.textSelectionStart >= 0 && node.textSelectionEnd >= 0) return Field.TEXT
            val cursorActions = node.actionList.any {
                it.id == AccessibilityNodeInfo.ACTION_SET_SELECTION ||
                    it.id == AccessibilityNodeInfo.ACTION_NEXT_AT_MOVEMENT_GRANULARITY
            }
            val hintSized = text.length <= HINT_MAX && '\n' !in text
            return when {
                !cursorActions && hintSized -> Field.EMPTY
                else -> Field.UNSURE
            }
        }

        /** Hints are short, one line: longer text is never taken for one. */
        private const val HINT_MAX = 64

        /** NFKC, lower case, without whitespace, format characters (U+200B, U+FEFF, ...) or control characters. */
        fun normalized(value: String): String =
            Normalizer.normalize(value, Normalizer.Form.NFKC).lowercase().filterNot { char ->
                char.isWhitespace() || Character.getType(char).let {
                    it == Character.FORMAT.toInt() || it == Character.CONTROL.toInt()
                }
            }

        /**
         * Field facts for the log, never the user's words: the lengths, whether the text matches the hint, and
         * where they first differ (two character codes from the hint, which is the app's own label).
         */
        private fun describe(node: AccessibilityNodeInfo): String {
            val text = node.text?.toString().orEmpty()
            val hint = node.hintText?.toString()
            val mismatch = if (hint == null || text == hint) {
                ""
            } else {
                val at = text.indices.firstOrNull { it >= hint.length || text[it] != hint[it] } ?: hint.length
                if (text.length == hint.length || at < minOf(text.length, hint.length)) {
                    " firstDifference=$at (text U+%04X, hint U+%04X)".format(
                        text.getOrNull(at)?.code ?: 0,
                        hint.getOrNull(at)?.code ?: 0,
                    )
                } else {
                    ""
                }
            }
            val actions = node.actionList.map { it.id }
            return "${node.packageName}/${node.className} textLength=${text.length} hintLength=${hint?.length ?: -1} " +
                "showingHint=${node.isShowingHintText} equalsHint=${text == hint}$mismatch " +
                "selection=${node.textSelectionStart}..${node.textSelectionEnd} " +
                "setSelection=${AccessibilityNodeInfo.ACTION_SET_SELECTION in actions} " +
                "granularity=${AccessibilityNodeInfo.ACTION_NEXT_AT_MOVEMENT_GRANULARITY in actions} " +
                "paste=${AccessibilityNodeInfo.ACTION_PASTE in actions}"
        }

        /** Adds a space where the dictated text would touch a word before or after it. */
        fun spaced(before: String, text: String, after: String): String {
            if (text.isEmpty()) return text
            val lead = if (before.isNotEmpty() && !before.last().isWhitespace() && !text.first().isWhitespace()) " " else ""
            val trail = if (after.isNotEmpty() && !after.first().isWhitespace() && after.first() !in ".,;:!?") " " else ""
            return lead + text + trail
        }
    }
}
