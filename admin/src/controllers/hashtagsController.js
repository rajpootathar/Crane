const { response } = require("../utils/response-handler");
const { RESPONSE_STATUS } = require("../config/constants");
const HashtagService = require("../services/hashtag-service");


const HashtagsController = () => {};
const service = new HashtagService();
HashtagsController.fetchAllHashtags = async (req, res,next) => {
    const {page=1,pageSize=10}=req.params;
    try {
        const data = await service.getAllHashtags(page,pageSize);
        return response(res, RESPONSE_STATUS.OK.code, true, data, null);
    }
    catch (error) {
        next(error);
    }
}

HashtagsController.deleteHashtag = async (req, res,next) => {
    const {id}= req.params;
    try {
        const data = await service.deleteHashtag(id);
        return response(res, RESPONSE_STATUS.OK.code, true, data, null);
    }
    catch (error) {
        next(error);
    }
}
HashtagsController.getHashtagDetails = async (req, res,next) => {
    const {id}= req.params;
    try {
        const data = await service.getHashtagDetails(id);
        return response(res, RESPONSE_STATUS.OK.code, true, data, null);
    }
    catch (error) {
        next(error);
    }
}

module.exports = HashtagsController;
