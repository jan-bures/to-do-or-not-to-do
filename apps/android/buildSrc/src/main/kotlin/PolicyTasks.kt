import org.gradle.api.DefaultTask
import org.gradle.api.provider.Property
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.TaskAction

/**
 * Fails a variant's build unless plain http to loopback is allowed exactly in the `debug` build type: the
 * `ALLOW_LOOPBACK_HTTP` build config field must be `true` there and `false` everywhere else, whatever else
 * (such as `isDebuggable`) the build type sets.
 */
abstract class VerifyCleartextPolicy : DefaultTask() {
    @get:Input
    abstract val buildType: Property<String>

    @get:Input
    abstract val allowLoopbackHttp: Property<String>

    @TaskAction
    fun verify() {
        val expected = (buildType.get() == "debug").toString()
        check(allowLoopbackHttp.get() == expected) {
            "ALLOW_LOOPBACK_HTTP is ${allowLoopbackHttp.get()} in the ${buildType.get()} build type; it must be $expected " +
                "(only the debug build type may talk plain http, and only to loopback)"
        }
    }
}
