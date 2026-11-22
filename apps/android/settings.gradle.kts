// The Android app. Its Rust core is crates/todo-ffi in the root Cargo workspace (two levels up); the
// build compiles it with cargo-ndk and generates the Kotlin bindings, see app/build.gradle.kts.
pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
        // The Kotlin half of rustls-platform-verifier, which the Rust TLS stack calls on Android. Only
        // its own group is looked up there.
        exclusiveContent {
            forRepository {
                maven {
                    name = "rustls-platform-verifier"
                    url = uri("https://github.com/rustls/rustls-platform-verifier/raw/maven-archive/android-release-support/maven/")
                }
            }
            filter { includeGroup("org.rustls") }
        }
    }
}

rootProject.name = "to-do-or-not-to-do"
include(":app")
