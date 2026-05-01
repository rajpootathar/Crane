const { Router } = require("express");
const ReelsController = require("../controllers/reelsController");

const router = Router();
router.get("/", ReelsController.fetchAllReels); // Get All Reels List API
router.delete("/:id", ReelsController.deleteReel); // Delete Reels List API
router.get("/:id", ReelsController.getReelDetails); // Get Reels details API

module.exports = router;
