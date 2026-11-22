package io.github.hi9841.tdt.model

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext

/** One model file, pinned by SHA-256. */
data class ModelFile(val name: String, val sha256: String, val bytes: Long)

/**
 * The files of one Trit model on Hugging Face, and how Trit holds its weights ([precision]). The files match TDT
 * desktop (desktop/src/stt/models.rs).
 */
data class ModelPack(
    val repo: String,
    val revision: String,
    val dir: String,
    val files: List<ModelFile>,
    val precision: String,
    /**
     * Digital quiet that the model gets after the audio of each read that ends a take, as on TDT desktop 0.9.10.
     * Parakeet Unified English writes a word about 0.3 s after the word ends: it learned to stream with right
     * context. A take that ends at its last word then loses that word, and a one-word take can give no text.
     */
    val endQuietMillis: Int = 0,
) {
    val totalBytes = files.sumOf { it.bytes }

    companion object {
        /** Parakeet Redux: the same weights as TDT desktop. Trit reads these three files (desktop also keeps ternary.json). */
        val Redux = ModelPack(
            repo = "moondream/parakeet-redux",
            // The commit of `main` on 2026-11-09. With "main", a new commit in the repository fails the checksums.
            revision = "2bf128600aac4b16946f7ed8372e56117fe5e23b",
            dir = "parakeet-redux",
            files = listOf(
                ModelFile("config.json", "503C653B2E3BB788ADBCB04F5ABDEE532D958686564081BAEED133FF10143F6E", 12_988),
                ModelFile("model.safetensors", "78EC25733EE0D0C1586D1346FC86DB9D0C2E436E3A8AB1D32A82D1BB8F848D21", 177_774_490),
                ModelFile("tokenizer.json", "BD321B096832A3F270BD3B2A88823957920F1A5C5ADA71114A26EA729D0CBE91", 1_159_960),
            ),
            precision = "auto",
            // The quiet cost Redux words on TDT desktop: other 173 to 183 errors.
            endQuietMillis = 0,
        )

        /** NVIDIA Parakeet Unified English 0.6B in ggml's Q8_0, the file TDT desktop 0.8 uses, at its pinned revision. */
        val Unified = ModelPack(
            repo = "handy-computer/parakeet-unified-en-0.6b-gguf",
            revision = "7e948f21b7bdbac698d3318db9d350f1096f3b6c",
            dir = "parakeet-unified-en-gguf",
            files = listOf(
                ModelFile(
                    "parakeet-unified-en-0.6b-Q8_0.gguf",
                    "4B50B6DD862BF6E346929AAF4F5EAACEC003BFA3F56462D6C874B41EF2F38795",
                    731_357_568,
                ),
            ),
            // "int8", as on TDT desktop: int8 weights in groups of 128, so the int8 kernel spends less time on scales.
            // OnePlus 13R, 6 cores (2026-11-17): the kernel at 439 GMAC/s against 290 for "q8_0" (the stored blocks of
            // 32), five takes read in 1,426-1,533 ms against 1,586-2,086 ms. Load 1.1 s against 0.55 s. Word errors on
            // 632 reference clips: 1,405 against 1,426 for "q8_0" and 1,429 for "auto".
            precision = "int8",
            // TDT desktop, 309 short LibriSpeech phrases that end 60 ms after the last word: 52 to 27 errors in
            // 1,489 words, 7 to 0 takes with no text. The long sets stayed the same.
            endQuietMillis = 480,
        )
    }
}

/** True on Wi-Fi or another network the user does not pay per byte for. */
fun isUnmetered(context: Context): Boolean {
    val manager = context.getSystemService(ConnectivityManager::class.java)
    val capabilities = manager.getNetworkCapabilities(manager.activeNetwork) ?: return false
    return capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED)
}

/** Downloads, checks and finds one [ModelPack] in app storage. */
class ModelStore(context: Context, val pack: ModelPack) {
    val dir: File = File(context.filesDir, "models/${pack.dir}")

    /** Checks sizes only, so it is cheap enough for every launch. Downloads verify SHA-256. */
    fun isInstalled(): Boolean = pack.files.all { File(dir, it.name).length() == it.bytes }

    /** Downloads missing files. [progress] gets bytes done of [ModelPack.totalBytes]. */
    suspend fun download(progress: (Long) -> Unit) = withContext(Dispatchers.IO) {
        dir.mkdirs()
        var done = 0L
        for (file in pack.files) {
            val target = File(dir, file.name)
            if (target.length() == file.bytes) {
                done += file.bytes
                progress(done)
                continue
            }
            val part = File(dir, "${file.name}.part")
            val digest = MessageDigest.getInstance("SHA-256")
            val url = URL("https://huggingface.co/${pack.repo}/resolve/${pack.revision}/${file.name}?download=true")
            val connection = url.openConnection() as HttpURLConnection
            try {
                connection.connectTimeout = 15_000
                connection.readTimeout = 30_000
                check(connection.responseCode == HttpURLConnection.HTTP_OK) {
                    "Download of ${file.name} failed: HTTP ${connection.responseCode}"
                }
                connection.inputStream.use { input ->
                    part.outputStream().use { output ->
                        val buffer = ByteArray(1 shl 16)
                        while (true) {
                            ensureActive()
                            val read = input.read(buffer)
                            if (read < 0) break
                            output.write(buffer, 0, read)
                            digest.update(buffer, 0, read)
                            done += read
                            progress(done)
                        }
                    }
                }
            } finally {
                connection.disconnect()
            }
            val actual = digest.digest().joinToString("") { "%02X".format(it) }
            if (actual != file.sha256) {
                part.delete()
                error("${file.name} is damaged (SHA-256 mismatch). Try the download again.")
            }
            check(part.renameTo(target)) { "Could not save ${file.name}." }
        }
    }
}
