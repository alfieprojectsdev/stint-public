import { Octokit } from "@octokit/rest";
import { toMarkdown } from "./to-markdown.js";

export async function postPRComment(reviewResult) {
    const token = process.env.GITHUB_TOKEN;
    const repo = process.env.REPO;
    const prNumber = Number(process.env.PR_NUMBER);

    if (!token || !repo || !prNumber) {
        throw new Error("Missing GITHUB_TOKEN, REPO, or PR_NUMBER");
    }

    const [owner, repoName] = repo.split("/");
    const octokit = new Octokit({ auth: token });

    // Findings with a real file path and positive line number can be posted as
    // inline review comments; everything else goes into the summary body.
    const inlineFindings = reviewResult.findings.filter(
        (f) => f.file_path && f.file_path !== "N/A" && f.line_number > 0,
    );

    if (inlineFindings.length > 0) {
        try {
            const { data: pr } = await octokit.pulls.get({
                owner,
                repo: repoName,
                pull_number: prNumber,
            });

            const inlineComments = inlineFindings.map((f) => ({
                path: f.file_path,
                line: f.line_number,
                side: "RIGHT",
                body: `**[${f.severity.toUpperCase()}] ${f.title}**\n\n${f.summary}\n\n> ${f.evidence}\n\n**Fix:** ${f.recommendations}`,
            }));

            await octokit.pulls.createReview({
                owner,
                repo: repoName,
                pull_number: prNumber,
                commit_id: pr.head.sha,
                body: toMarkdown(reviewResult),
                event: "COMMENT",
                comments: inlineComments,
            });
            return;
        } catch (err) {
            // Inline review failed (e.g. line numbers outside the diff).
            // Fall through to a plain issue comment so the review is never silently dropped.
            console.warn(`Inline review failed, falling back to issue comment: ${err.message}`);
        }
    }

    await octokit.issues.createComment({
        owner,
        repo: repoName,
        issue_number: prNumber,
        body: toMarkdown(reviewResult),
    });
}
