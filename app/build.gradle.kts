import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

// Release signing comes from keystore.properties (gitignored). Without it, release builds are unsigned.
val keystore = rootProject.file("keystore.properties").takeIf { it.isFile }?.let { file ->
    Properties().apply { file.inputStream().use(::load) }
}

android {
    namespace = "io.github.hi9841.tdt"
    // -Ptdt.testBuildType=release signs the engine test with the release key, so it runs inside an installed
    // release build and its downloaded model.
    testBuildType = providers.gradleProperty("tdt.testBuildType").getOrElse("debug")
    compileSdk = 37
    ndkVersion = "27.1.12297006"

    defaultConfig {
        applicationId = "io.github.hi9841.tdt"
        minSdk = 29
        targetSdk = 37
        versionCode = 22
        versionName = "0.6.10"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"

        externalNativeBuild {
            cmake {
                // NDK 27 needs this for Android 15+ phones with 16 KB pages.
                arguments += "-DANDROID_SUPPORT_FLEXIBLE_PAGE_SIZES=ON"
                targets += "tdt_engine"
            }
        }
    }

    signingConfigs {
        if (keystore != null) {
            create("release") {
                storeFile = file(keystore.getProperty("storeFile"))
                storePassword = keystore.getProperty("storePassword")
                keyAlias = keystore.getProperty("keyAlias")
                keyPassword = keystore.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        debug {
            // x86_64 runs the engine on the emulator; phones use arm64-v8a.
            ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
            buildConfigField("boolean", "BUBBLE", "true")
        }
        release {
            isMinifyEnabled = false
            ndk { abiFilters += listOf("arm64-v8a") }
            signingConfig = signingConfigs.findByName("release")
            buildConfigField("boolean", "BUBBLE", "true")
        }
        // The release without the bubble's Accessibility service. Play Protect's fraud protection blocks apps that
        // declare one when they come from a browser or a file manager, with no "Install anyway" in some countries;
        // this build installs from a link and keeps in-app dictation. src/sideload/AndroidManifest.xml removes the
        // service.
        create("sideload") {
            initWith(getByName("release"))
            buildConfigField("boolean", "BUBBLE", "false")
        }
    }

    externalNativeBuild {
        cmake {
            path = file("src/main/cpp/CMakeLists.txt")
            version = "4.1.2"
        }
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }
}

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.foundation)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.ui.tooling.preview)
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.desertant.voz)
    debugImplementation(libs.androidx.compose.ui.tooling)
    testImplementation(libs.junit)
    androidTestImplementation(libs.androidx.test.junit)
    androidTestImplementation(libs.androidx.test.runner)
}
