package io.github.hi9841.tdt

import android.content.Intent
import android.net.Uri
import android.os.SystemClock
import android.view.accessibility.AccessibilityNodeInfo
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.hi9841.tdt.bubble.InsertResult
import io.github.hi9841.tdt.bubble.TextInserter
import org.junit.Assert.assertEquals
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Inserts into another app's real text field (Google Messages) through the accessibility API, the same calls the
 * bubble makes. UiAutomation stands in for the bubble's Accessibility service.
 */
@RunWith(AndroidJUnit4::class)
class InserterTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val automation = instrumentation.uiAutomation

    @Test
    fun insertsAtTheCursorWithSpacingAndKeepsTheClipboardOut() {
        val field = openMessagesField()
        val inserter = TextInserter(instrumentation.targetContext)

        assertEquals(InsertResult.SET_TEXT, inserter.insert(field, "Hello from TDT."))
        assertEquals(InsertResult.SET_TEXT, inserter.insert(field, "Second sentence."))
        field.refresh()
        assertEquals("Hello from TDT. Second sentence.", field.text.toString())
    }

    private fun openMessagesField(): AccessibilityNodeInfo {
        val context = instrumentation.targetContext
        val intent = Intent(Intent.ACTION_SENDTO, Uri.parse("sms:5550100"))
            .setPackage("com.google.android.apps.messaging")
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
        val started = runCatching { context.startActivity(intent) }.isSuccess
        assumeTrue("Google Messages is not installed", started)

        val deadline = SystemClock.uptimeMillis() + 10_000
        while (SystemClock.uptimeMillis() < deadline) {
            val field = automation.rootInActiveWindow?.let(::findEditable)
            if (field != null) {
                field.performAction(AccessibilityNodeInfo.ACTION_FOCUS)
                field.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, null) // Start from an empty draft.
                return field
            }
            SystemClock.sleep(250)
        }
        error("No editable field appeared in Messages")
    }

    private fun findEditable(node: AccessibilityNodeInfo): AccessibilityNodeInfo? {
        if (node.isEditable) return node
        for (i in 0 until node.childCount) node.getChild(i)?.let(::findEditable)?.let { return it }
        return null
    }
}
