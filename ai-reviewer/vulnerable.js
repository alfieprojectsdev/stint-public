app.get("/user", async (req, res) => {
    const result = await db.query(
        `SELECT * FROM users WHERE id = ${req.query.id}`,
    );
    res.json(result.rows);
});
