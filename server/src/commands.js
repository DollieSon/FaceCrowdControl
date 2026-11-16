async function givePointsToUser(dbclient, sender, targetId, points) {
    // This function will give points to a user
    // `sender` is the sender's users row; they're identified by id, never by display name.
    const userName = sender.name;
    console.log(`Giving ${points} points from ${userName} to ${targetId}`);

    // Accept "42" or "#42" as the target id.
    targetId = String(targetId).replace(/^#/, '');
    if (!/^\d+$/.test(targetId)) {
        throw new Error('Target must be a user id number, e.g. /give #42 10');
    }
    // Whole positive amounts only: a negative amount would take points from the target.
    if (!/^\d+$/.test(String(points)) || parseInt(points, 10) <= 0) {
        throw new Error('Amount must be a positive whole number');
    }
    points = parseInt(points, 10);
    targetId = parseInt(targetId, 10);

    try {
        const text = `SELECT id, point FROM users WHERE id = $1`;
        const values = [sender.id];

        const result = await dbclient.query(text, values);
        if (result.rows.length === 0) {
            throw new Error('User not found');
        }
        const userPoints = result.rows[0].point;        // check if points is enough
        if (userPoints < points) {
            throw new Error('Not enough points');
        }
        const userId = result.rows[0].id;
        // Get target user by id
        // const findTargetText = `SELECT id FROM users WHERE name = $1`;
        // const findTargetValues = [targetId];
        // const targetResult = await dbclient.query(findTargetText, findTargetValues);
        // if (targetResult.rows.length === 0) {
        //     throw new Error('Target user not found');
        // }
        // const targetUserId = targetResult.rows[0].id;
        // const userIdResult = await dbclient.query('SELECT id FROM users WHERE name = $1', [userName]);
        // const userId = userIdResult.rows[0].id;
        
        // if enough points, deduct points from user and give to targetId
        const updateText = `
            UPDATE users
            SET 
            point = CASE 
                WHEN id = $1 THEN point - $3
                WHEN id = $2 THEN point + $3
                ELSE point
            END,
            latestInteraction = NOW()
            WHERE id IN ($1, $2)
            RETURNING *;
        `;
        const updateValues = [userId, targetId, points];
        const updateResult = await dbclient.query(updateText, updateValues);
        
        if (updateResult.rows.length === 0) {
            throw new Error('Update failed');
        }
        
        console.log(`${points} Points transferred from user ${userName} to target ${targetId}`);
        return { success: true, message: `${points} points transferred to ${targetId}` };
    } catch (error) {
        console.error('Error in givePointsToUser:', error);
        throw error;
    }
}

module.exports = {
    givePointsToUser
};