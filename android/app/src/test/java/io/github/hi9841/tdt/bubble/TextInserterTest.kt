package io.github.hi9841.tdt.bubble

import org.junit.Assert.assertEquals
import org.junit.Test

class TextInserterTest {
    @Test fun emptyFieldGetsTheTextAsIs() = assertEquals("Hello.", TextInserter.spaced("", "Hello.", ""))

    @Test fun aSpaceSeparatesTheTextFromTheWordBefore() = assertEquals(" more.", TextInserter.spaced("One", "more.", ""))

    @Test fun noExtraSpaceAfterExistingWhitespace() = assertEquals("more", TextInserter.spaced("One ", "more", ""))

    @Test fun aSpaceSeparatesTheTextFromTheWordAfter() = assertEquals("big ", TextInserter.spaced("", "big", "dog"))

    @Test fun punctuationAfterNeedsNoSpace() = assertEquals(" end", TextInserter.spaced("The", "end", "."))
}
