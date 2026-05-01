const { Router } = require("express");
const StoryController = require("../controllers/storiesController");

const router = Router();
router.get("/", StoryController.fetchAllStories); // Get All Reels List API
router.delete("/:id",StoryController.deleteStory); // Delete Reels List API
router.get("/:id", StoryController.getStoryDetails); // Get Reels details API

module.exports = router;
