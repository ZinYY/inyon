package tech.abhashchakraborty.inyon.device

import java.io.File
import javax.xml.parsers.DocumentBuilderFactory
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.w3c.dom.Element

/** The manifest and its XML, read as files: what Android is told about backups and transfers. */
class ManifestTest {
    private val android = "http://schemas.android.com/apk/res/android"

    private fun xml(path: String): Element {
        val f = DocumentBuilderFactory.newInstance().apply { isNamespaceAware = true }
        return f.newDocumentBuilder().parse(File(path)).documentElement
    }

    private fun application() = xml("src/main/AndroidManifest.xml").getElementsByTagName("application").item(0) as Element

    @Test
    fun backupsAndDeviceTransfersCopyNothing() {
        val app = application()
        assertEquals("false", app.getAttributeNS(android, "allowBackup"))
        assertEquals("@xml/data_extraction_rules", app.getAttributeNS(android, "dataExtractionRules"))
        val rules = xml("src/shared/res/xml/data_extraction_rules.xml")
        for (section in listOf("cloud-backup", "device-transfer")) {
            val s = rules.getElementsByTagName(section).item(0) as Element
            val excluded = (0 until s.getElementsByTagName("exclude").length).map { (s.getElementsByTagName("exclude").item(it) as Element).getAttribute("domain") }.toSet()
            assertTrue("$section leaves something in: $excluded", excluded.containsAll(listOf("root", "file", "database", "sharedpref", "external")))
            assertEquals("$section includes something", 0, s.getElementsByTagName("include").length)
        }
    }
}
