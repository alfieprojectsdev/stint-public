export function toMarkdown(reviewResult) {
    const { verdict, summary, findings } = reviewResult;

    let output = `## AI PR Review\n\n`;
    output += `**Verdict:** ${verdict}\n\n`;
    output += `**Summary:** ${summary}\n\n`;

    if (!findings.length) {
        output += `No findings were reported.\n`;
        return output;
    }

    output += `### Findings\n\n`;

    for (const finding of findings) {
        output += `- **${finding.title}**\n`;
        output += `  - Severity: ${finding.severity}\n`;
        output += `  - File: ${finding.file_path}\n`;
        output += `  - Line: ${finding.line_number}\n`;
        output += `  - Summary: ${finding.summary}\n`;
        output += `  - Evidence: ${finding.evidence}\n`;
        output += `  - Recommendation: ${finding.recommendations}\n\n`;
    }

    return output;
}
