import fs from "fs";
import { reviewCode } from "./review.js";
import { reviewJsonSchema, reviewSchema } from "./schema.js";
import { redactSecrets } from "./redact-secrets.js";
import { failClosedResult } from "./fail-closed-result.js";
import { postPRComment } from "./postPRComment.js";

const DIFF_CHAR_LIMIT = 4000;

async function main() {
    const isGitHubAction = process.env.GITHUB_ACTIONS === "true";

    // In Actions, prefer PR_DIFF env var when set; otherwise fall through to stdin.
    const diffText = (isGitHubAction && process.env.PR_DIFF)
        ? process.env.PR_DIFF
        : fs.readFileSync(0, "utf8");

    if (!diffText) {
        console.error("No diff text provided");
        process.exit(1);
    }

    const redactedDiff = redactSecrets(diffText);
    if (redactedDiff.length > DIFF_CHAR_LIMIT) {
        console.warn(`Diff truncated from ${redactedDiff.length} to ${DIFF_CHAR_LIMIT} characters`);
    }
    const limitedDiff = redactedDiff.slice(0, DIFF_CHAR_LIMIT);

    let validated;

    try {
        const result = await reviewCode(limitedDiff, reviewJsonSchema);
        const text = result?.content?.[0]?.text;
        if (typeof text !== "string") {
            throw new Error(`Unexpected API response shape: ${JSON.stringify(result?.content?.[0])}`);
        }
        const rawJson = JSON.parse(text);
        validated = reviewSchema.parse(rawJson);
    } catch (error) {
        validated = failClosedResult(error);
    }

    if (isGitHubAction) {
        await postPRComment(validated);
    } else {
        console.log(JSON.stringify(validated, null, 2));
    }
}

main().catch((error) => {
    console.error(error);
    process.exit(1);
});
