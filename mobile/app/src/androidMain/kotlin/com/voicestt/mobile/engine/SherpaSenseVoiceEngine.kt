package com.voicestt.mobile.engine

import android.content.Context
import com.k2fsa.sherpa.onnx.OfflineModelConfig
import com.k2fsa.sherpa.onnx.OfflineRecognizer
import com.k2fsa.sherpa.onnx.OfflineRecognizerConfig
import com.k2fsa.sherpa.onnx.OfflineSenseVoiceModelConfig
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File
import java.io.FileOutputStream

class SherpaSenseVoiceEngine(private val context: Context) {

    private var recognizer: OfflineRecognizer? = null
    private var isInitialized = false
    private val stateLock = Any()

    suspend fun initialize(): Boolean = withContext(Dispatchers.IO) {
        synchronized(stateLock) {
            if (isInitialized) return@withContext true

            try {
                // The model is copied and loaded only while dictation is active.
                val modelDir = File(context.filesDir, "sensevoice")
                if (!modelDir.exists()) {
                    modelDir.mkdirs()
                }

                val modelFile = File(modelDir, "model.onnx")
                val tokensFile = File(modelDir, "tokens.txt")

                if (!modelFile.exists() || !tokensFile.exists()) {
                    copyAssetFile("sensevoice/model.onnx", modelFile)
                    copyAssetFile("sensevoice/tokens.txt", tokensFile)
                }

                if (!modelFile.exists() || !tokensFile.exists()) {
                    return@withContext false
                }

                val senseVoiceConfig = OfflineSenseVoiceModelConfig(
                    model = modelFile.absolutePath,
                    language = "auto",
                    useInverseTextNormalization = true
                )

                val modelConfig = OfflineModelConfig(
                    senseVoice = senseVoiceConfig,
                    tokens = tokensFile.absolutePath,
                    numThreads = 2,
                    debug = false,
                    provider = "cpu",
                    modelType = "sense_voice"
                )

                recognizer = OfflineRecognizer(
                    assetManager = null,
                    config = OfflineRecognizerConfig(modelConfig = modelConfig)
                )
                isInitialized = true
                true
            } catch (e: Exception) {
                e.printStackTrace()
                false
            }
        }
    }

    private fun copyAssetFile(assetPath: String, destFile: File) {
        try {
            context.assets.open(assetPath).use { input ->
                FileOutputStream(destFile).use { output ->
                    input.copyTo(output)
                }
            }
        } catch (_: Exception) {
            // Asset file might be downloaded dynamically
        }
    }

    suspend fun transcribe(samples: FloatArray): Result<String> = withContext(Dispatchers.Default) {
        if (samples.isEmpty()) {
            return@withContext Result.success("")
        }

        if (!initialize()) {
            return@withContext Result.failure(
                IllegalStateException("SenseVoice model is unavailable. Build with the downloaded model assets.")
            )
        }

        try {
            val result = synchronized(stateLock) {
                val rec = recognizer ?: return@synchronized Result.failure<String>(
                    IllegalStateException("SenseVoice recognizer was released")
                )
                val stream = rec.createStream()
                stream.acceptWaveform(samples, 16000)
                rec.decode(stream)
                val text = rec.getResult(stream).text.trim()
                stream.release()
                Result.success(text)
            }
            result
        } catch (e: Exception) {
            Result.failure(e)
        } finally {
            release()
        }
    }

    fun release() {
        synchronized(stateLock) {
            recognizer?.release()
            recognizer = null
            isInitialized = false
        }
    }
}
