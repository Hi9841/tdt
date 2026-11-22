package io.github.hi9841.tdt.text

import org.junit.Assert.assertEquals
import org.junit.Test

/** The same cases as TDT desktop `desktop/src/stt/fillers.rs`. */
class FillersTest {
    @Test fun removesHesitationsAndTheCommasAroundThem() = assertEquals(
        "So I think we should go to the store, and buy some milk. You know, it's, like, really important.",
        Fillers.remove("So, um, I think we should, uh, go to the store, and, um, buy some milk. Uh, you know, it's, like, really important."),
    )

    @Test fun textWithoutFillersIsUnchanged() {
        val text = "Ship it  today.\nThen rest, I guess."
        assertEquals(text, Fillers.remove(text))
    }

    @Test fun aSentenceThatStartedOnAFillerKeepsItsCapital() {
        assertEquals("Yes.", Fillers.remove("Um, yes."))
        assertEquals("Fine. Next one?", Fillers.remove("Fine. Uh, next one?"))
        assertEquals("I want that.", Fillers.remove("Uh, uh, I want that."))
    }

    @Test fun aFillerAtTheEndHandsOverItsFullStop() {
        assertEquals("That's it.", Fillers.remove("That's it, um."))
        assertEquals("Is that all?", Fillers.remove("Is that all, uh?"))
        assertEquals("Done.", Fillers.remove("Done. Um."))
    }

    @Test fun repeatedAndStretchedFillersGoToo() {
        assertEquals("Hello", Fillers.remove("Um um umm ummm hello"))
        assertEquals("I was thinking", Fillers.remove("I was, uhhh, hmm, thinking"))
        assertEquals("Okay.", Fillers.remove("Erm, mmm, okay."))
    }

    @Test fun onlyFillersLeavesNothing() {
        assertEquals("", Fillers.remove("Um."))
        assertEquals("", Fillers.remove("uh, um..."))
    }

    @Test fun wordsThatCarryMeaningStay() {
        for (text in listOf(
            "I like it, you know.", "Uh-huh, that works.", "Mhm, sure.", "To err is human.",
            "Plan B or plan M.", "Ah, I see.", "The UMass campus.", "Hummus please.",
        )) assertEquals(text, Fillers.remove(text))
    }

    @Test fun quotesOpenedOnAFillerMoveToTheNextWord() =
        assertEquals("He said \"hello\" to me.", Fillers.remove("He said \"um, hello\" to me."))

    @Test fun listsKeepTheirColonAndLineBreaksSurvive() =
        assertEquals("Buy: eggs\nand bread", Fillers.remove("Buy: um, eggs\nand, uh, bread"))
}
