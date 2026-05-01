const { Router } = require("express");
const AdminUserController = require("../controllers/adminUserController");

const router = Router();

router.get("/", (req, res) => {
  res.status(200).send("server is up");
});
router.post("/register-admin", AdminUserController.createAccount); // Create Account API
router.get("/admin-user-list", AdminUserController.getAdminUser); // Get Admin User List API
router.get("/:id/admin-profile", AdminUserController.getAdminProfile); // Get Admin User profile by id API
router.patch("/:id/admin-profile", AdminUserController.updateAdminProfile); // update Admin User profile by id API
router.post("/login", AdminUserController.login); // Login API
// router.post("/refresh", AdminUserController.GetRefreshToken); // Get Refresh Token API
// router.post("/forgot-password", AdminUserController.ForgotPassword); // Post forgot password
router.post("/:id/change-password", AdminUserController.changePassword); // Post change password
// router.post("/logout", AdminUserController.logout); // Post logout user
// router.post("/delete-account", AdminUserController.deleteAccount); // Post logout user

module.exports = router;
