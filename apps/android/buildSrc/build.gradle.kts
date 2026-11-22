plugins {
    `kotlin-dsl`
}

repositories {
    mavenCentral()
}

dependencies {
    // Tests for the build logic (the brand icon converter): `./gradlew -p buildSrc test`, run by CI.
    testImplementation("junit:junit:4.13.2")
}
