const { Router } = require("express");
// const auth = require("../middleware/auth");
const DashboardController = require("../controllers/dashboardController");

const router = Router();

router.get("/", (req, res) => {
  res.status(200).send("Dashbaord service");
});

router.post('/user-metrix', DashboardController.getUserMatrix)
router.post('/content-metrix', DashboardController.getContentMetrics)
router.post('/recent-user-list', DashboardController.recentUserList)
router.post('/uploaded-post-metrix', DashboardController.getUploadedPostMetrix)
router.post('/user-registration-metrix', DashboardController.getUserRegistrationMetrix)



module.exports = router;