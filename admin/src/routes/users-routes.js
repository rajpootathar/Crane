const { Router } = require("express");
const UsersController = require("../controllers/usersController");

const router = Router();
router.get("/", UsersController.fetchUsers); // Get User List API
router.get("/:id", UsersController.getUserProfile); // Get User by id API
router.delete("/:userId", UsersController.deleteUserAccount); // Delete User by id API
router.put("/status/:userId", UsersController.ActivateDisableUserAccount);
module.exports = router;
