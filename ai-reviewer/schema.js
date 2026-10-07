import { z } from "zod";

export const reviewJsonSchema = {
    type: "object",
    properties: {
        verdict: {
            type: "string",
            enum: ["pass", "warn", "fail"],
        },
        summary: {
            type: "string",
        },
        findings: {
            type: "array",
            items: {
                type: "object",
                properties: {
                    id: { type: "string" },
                    title: { type: "string" },
                    severity: {
                        type: "string",
                        enum: ["none", "low", "medium", "high", "critical"],
                        description:
                            "The severity level of the security or code issue",
                    },
                    summary: { type: "string" },
                    file_path: { type: "string" },
                    line_number: { type: "number" },
                    evidence: { type: "string" },
                    recommendations: { type: "string" },
                },
                required: [
                    "id",
                    "title",
                    "severity",
                    "summary",
                    "file_path",
                    "line_number",
                    "evidence",
                    "recommendations",
                ],
                additionalProperties: false,
            },
        },
    },
    required: ["verdict", "summary", "findings"],
    additionalProperties: false,
};

const findingSchema = z.object({
    id: z.string(),
    title: z.string(),
    severity: z.enum(["none", "low", "medium", "high", "critical"]),
    summary: z.string(),
    file_path: z.string(),
    line_number: z.number(),
    evidence: z.string(),
    recommendations: z.string(),
});

export const reviewSchema = z.object({
    verdict: z.enum(["pass", "warn", "fail"]),
    summary: z.string(),
    findings: z.array(findingSchema),
});
