/**
 * A small JSON reader for the token export: objects become `Map<String, Any?>` (in file order), arrays
 * `List<Any?>`, numbers `Double`. Enough for `tokens.json`; it isn't a general-purpose parser.
 */
class Json private constructor(private val s: String) {
    private var i = 0

    companion object {
        fun parse(text: String): Any? {
            val p = Json(text)
            val v = p.value()
            p.ws()
            require(p.i == p.s.length) { "trailing characters at ${p.i}" }
            return v
        }
    }

    private fun ws() {
        while (i < s.length && s[i].isWhitespace()) i++
    }

    private fun value(): Any? {
        ws()
        return when (val c = s[i]) {
            '{' -> obj()
            '[' -> arr()
            '"' -> str()
            't' -> literal("true", true)
            'f' -> literal("false", false)
            'n' -> literal("null", null)
            else -> if (c == '-' || c.isDigit()) num() else error("unexpected '$c' at $i")
        }
    }

    private fun literal(word: String, v: Any?): Any? {
        require(s.startsWith(word, i)) { "bad literal at $i" }
        i += word.length
        return v
    }

    private fun num(): Double {
        val start = i
        while (i < s.length && (s[i].isDigit() || s[i] in "+-.eE")) i++
        return s.substring(start, i).toDouble()
    }

    private fun str(): String {
        val sb = StringBuilder()
        i++
        while (s[i] != '"') {
            if (s[i] == '\\') {
                i++
                when (val e = s[i]) {
                    'n' -> sb.append('\n')
                    't' -> sb.append('\t')
                    'r' -> sb.append('\r')
                    'b' -> sb.append('\b')
                    'f' -> sb.append('\u000c')
                    'u' -> {
                        sb.append(s.substring(i + 1, i + 5).toInt(16).toChar())
                        i += 4
                    }
                    else -> sb.append(e)
                }
            } else {
                sb.append(s[i])
            }
            i++
        }
        i++
        return sb.toString()
    }

    private fun obj(): Map<String, Any?> {
        val m = LinkedHashMap<String, Any?>()
        i++
        ws()
        if (s[i] == '}') {
            i++
            return m
        }
        while (true) {
            ws()
            val k = str()
            ws()
            require(s[i] == ':') { "expected ':' at $i" }
            i++
            m[k] = value()
            ws()
            if (s[i] == ',') {
                i++
                continue
            }
            require(s[i] == '}') { "expected '}' at $i" }
            i++
            return m
        }
    }

    private fun arr(): List<Any?> {
        val l = ArrayList<Any?>()
        i++
        ws()
        if (s[i] == ']') {
            i++
            return l
        }
        while (true) {
            l.add(value())
            ws()
            if (s[i] == ',') {
                i++
                continue
            }
            require(s[i] == ']') { "expected ']' at $i" }
            i++
            return l
        }
    }
}
