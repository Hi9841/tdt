import org.gradle.api.tasks.Sync

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.multiplatform")
    id("org.jetbrains.compose")
    id("org.jetbrains.kotlin.plugin.compose")
}

// Keep the large model out of source control while making local and CI builds
// deterministic after `models/download-models.ps1` has been run.
val prepareSenseVoiceAssets by tasks.registering(Sync::class) {
    val modelDir = rootProject.file("../models/sensevoice")
    from(modelDir) {
        include("model.int8.onnx")
        rename("model.int8.onnx", "model.onnx")
    }
    from(modelDir) { include("tokens.txt") }
    into(layout.buildDirectory.dir("generated/sensevoice-assets/sensevoice"))
    doFirst {
        require(File(modelDir, "model.int8.onnx").isFile) {
            "Missing ../models/sensevoice/model.int8.onnx. Run models/download-models.ps1 first."
        }
        require(File(modelDir, "tokens.txt").isFile) {
            "Missing ../models/sensevoice/tokens.txt. Run models/download-models.ps1 first."
        }
    }
}

kotlin {
    androidTarget {
        compilations.all {
            kotlinOptions {
                jvmTarget = "21"
            }
        }
    }

    sourceSets {
        val commonMain by getting {
            dependencies {
                implementation(compose.runtime)
                implementation(compose.foundation)
                implementation(compose.material3)
                implementation(compose.ui)
                implementation(compose.components.resources)
                implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.8.1")
            }
        }

        val androidMain by getting {
            dependencies {
                implementation("androidx.activity:activity-compose:1.9.1")
                implementation("androidx.core:core-ktx:1.13.1")
                implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.4")
                implementation("androidx.lifecycle:lifecycle-service:2.8.4")
                implementation(files("libs/sherpa-onnx-1.13.8.aar"))
            }
        }
    }
}

android {
    namespace = "com.voicestt.mobile"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.voicestt.mobile"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "1.0.0"

        ndk {
            abiFilters.addAll(listOf("armeabi-v7a", "arm64-v8a", "x86", "x86_64"))
        }
    }

    buildFeatures {
        compose = true
    }

    sourceSets.getByName("main").assets.srcDir(
        layout.buildDirectory.dir("generated/sensevoice-assets")
    )

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_21
        targetCompatibility = JavaVersion.VERSION_21
    }

    packaging {
        resources {
            excludes += "/META-INF/{AL2.0,LGPL2.1}"
        }
    }
}

tasks.matching { task ->
    task.name == "mergeDebugAssets" || task.name == "mergeReleaseAssets"
}.configureEach {
    dependsOn(prepareSenseVoiceAssets)
}
