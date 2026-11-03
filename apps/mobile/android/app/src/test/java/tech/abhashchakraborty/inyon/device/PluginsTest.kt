package tech.abhashchakraborty.inyon.device

import java.io.File
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Every Capacitor plugin in the app, read from the sources: the bridge only knows the ones
 * MainActivity registers, and a release build's R8 strips the methods it calls by name unless
 * proguard-rules.pro keeps them.
 */
class PluginsTest {
    private val sources = File("src/main/java")

    private fun plugins(): List<String> =
        sources.walk().filter { it.extension == "kt" && it.readText().contains("@CapacitorPlugin(") }.map { f ->
            val pkg = Regex("""^package (\S+)""", RegexOption.MULTILINE).find(f.readText())!!.groupValues[1]
            "$pkg.${f.nameWithoutExtension}"
        }.toList()

    @Test
    fun eachPluginIsRegisteredAndKeptWhole() {
        val found = plugins()
        assertTrue("found $found", found.size >= 3 && found.any { it.endsWith(".InyonMigratePlugin") })
        val activity = File(sources, "tech/abhashchakraborty/inyon/MainActivity.kt").readText()
        val rules = File("proguard-rules.pro").readText()
        for (p in found) {
            val name = p.substringAfterLast('.')
            assertTrue("$name isn't registered in MainActivity", activity.contains("registerPlugin($name::class.java)"))
            assertTrue("$p isn't kept in proguard-rules.pro", rules.contains("-keep class $p { *; }"))
        }
    }

    @Test
    fun theUpgradeFileReaderIsKeptForTheAppThatInstallsOverThisOne() {
        assertTrue(File("proguard-rules.pro").readText().contains("-keep class tech.abhashchakraborty.inyon.migrate.MigrationExport { *; }"))
    }
}
