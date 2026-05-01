const { Router } = require("express");
const HashtagsController = require("../controllers/hashtagsController");

const router = Router();
router.get("/", HashtagsController.fetchAllHashtags); // Get All Hashtags List API
router.delete("/:id", HashtagsController.deleteHashtag); // Delete Hashtags List API
router.get("/:id", HashtagsController.getHashtagDetails); // Get Hashtags details API

module.exports = router;
