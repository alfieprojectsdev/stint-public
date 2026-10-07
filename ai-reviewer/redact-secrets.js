const SECRET_PATTERNS = [
    // Key=value assignments with sensitive variable names (env files, config, source code)
    /(?:api[_-]?key|api[_-]?secret|auth[_-]?token|access[_-]?token|secret[_-]?key|client[_-]?secret|private[_-]?key|password|passwd)\s*[:=]\s*["']?[A-Za-z0-9\-._~+/=]{8,}["']?/gi,
    // Bearer / Authorization header values
    /Bearer\s+[A-Za-z0-9\-._~+/=]{8,}/g,
    // Stripe keys (sk_live_, sk_test_, rk_live_, rk_test_, pk_live_, pk_test_)
    /\b(?:sk|rk|pk)_(?:live|test)_[A-Za-z0-9]{10,}\b/g,
    // GitHub personal access tokens and fine-grained tokens
    /\b(?:ghp|ghs|gho|ghu)_[A-Za-z0-9]{20,}\b/g,
    /\bgithub_pat_[A-Za-z0-9_]{20,}\b/g,
    // Google API keys (AIza...)
    /\bAIza[0-9A-Za-z_-]{35}\b/g,
    // AWS access key IDs
    /\bAKIA[0-9A-Z]{16}\b/g,
    // AWS secret access key values (only when following a known variable name)
    /(?:aws_secret_access_key|AWS_SECRET_ACCESS_KEY)\s*[:=]\s*["']?[A-Za-z0-9/+=]{40}["']?/gi,
    // Anthropic SDK keys (sk-ant-...)
    /\bsk-ant-[A-Za-z0-9\-]{20,}\b/g,
    // OpenAI keys (sk- or sk-proj-)
    /\bsk-(?:proj-)?[A-Za-z0-9\-]{20,}\b/g,
    // PEM private key blocks
    /-----BEGIN\s+(?:[A-Z ]+\s+)?PRIVATE KEY-----[\s\S]+?-----END\s+(?:[A-Z ]+\s+)?PRIVATE KEY-----/g,
    // URL credentials (https://user:password@host)
    /https?:\/\/[^:@/\s]+:[^@\s]+@[^\s"')]+/g,
    // JWT tokens (three base64url segments joined by dots)
    /\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b/g,
];

export function redactSecrets(input) {
    let output = input;

    for (const pattern of SECRET_PATTERNS) {
        output = output.replace(pattern, "[REDACTED]");
    }

    return output;
}
